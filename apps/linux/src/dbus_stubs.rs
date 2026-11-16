//! Private D-Bus config writer, silent `org.freedesktop.Notifications` stub,
//! and Xwayland `AT_SPI_BUS` root window property injection.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::wrapper::ConnectionExt as _;
use zbus::zvariant::OwnedValue;

pub fn write_isolated_dbus_config(config_path: &Path, socket_path: &Path) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    // Omit <standard_session_servicedirs/> so isolated clients never activate
    // host portals, keyrings, or AT-SPI services over systemd/D-Bus.
    let xml = format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <deny send_destination="*" eavesdrop="true"/>
    <deny eavesdrop="true"/>
    <allow user="{uid}"/>
    <allow own="*"/>
    <allow send_type="method_call"/>
    <allow send_type="signal"/>
    <allow send_requested_reply="true" send_type="method_return"/>
    <allow send_requested_reply="true" send_type="error"/>
    <allow receive_type="method_call"/>
    <allow receive_type="method_return"/>
    <allow receive_type="error"/>
    <allow receive_type="signal"/>
  </policy>
</busconfig>
"#,
        socket_path.display()
    );
    std::fs::write(config_path, xml)?;
    Ok(())
}

pub fn set_xwayland_atspi_bus(x11_display: &str, bus_address: &str) -> Result<()> {
    if x11_display.is_empty() {
        return Ok(());
    }
    let (conn, screen_num) = x11rb::connect(Some(x11_display))
        .with_context(|| format!("connect to Xwayland {x11_display}"))?;
    let root = conn.setup().roots[screen_num].root;
    let atom = conn
        .intern_atom(false, b"AT_SPI_BUS")?
        .reply()
        .context("intern AT_SPI_BUS")?
        .atom;
    conn.change_property8(
        PropMode::REPLACE,
        root,
        atom,
        AtomEnum::STRING,
        bus_address.as_bytes(),
    )?
    .check()
    .context("ChangeProperty AT_SPI_BUS")?;
    conn.sync()?;
    Ok(())
}

#[derive(Default)]
pub struct NotificationStats {
    pub count: AtomicU32,
}

struct SilentNotifications {
    stats: Arc<NotificationStats>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl SilentNotifications {
    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".to_string()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "agent-screen-notifications".to_string(),
            "AgentScreen".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        _summary: &str,
        _body: &str,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        if replaces_id > 0 {
            replaces_id
        } else {
            self.stats.count.fetch_add(1, Ordering::Relaxed) + 1
        }
    }

    fn close_notification(&self, _id: u32) {}
}

pub async fn spawn_silent_notifications(
    session_bus_addr: &str,
) -> Result<(zbus::Connection, Arc<NotificationStats>)> {
    let stats = Arc::new(NotificationStats::default());
    let svc = SilentNotifications {
        stats: stats.clone(),
    };
    let conn = zbus::connection::Builder::address(session_bus_addr)?
        .name("org.freedesktop.Notifications")?
        .serve_at("/org/freedesktop/Notifications", svc)?
        .build()
        .await
        .context("bind org.freedesktop.Notifications on display session bus")?;
    Ok((conn, stats))
}
