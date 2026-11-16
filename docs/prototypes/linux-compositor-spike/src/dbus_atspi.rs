//! Isolated D-Bus session bus, private AT-SPI2 bus, Xwayland `AT_SPI_BUS` root
//! window property injection, and silent `org.freedesktop.Notifications` stub.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::wrapper::ConnectionExt as _;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

pub fn write_dbus_config(config_path: &Path, socket_path: &Path) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    // Deliberately omits <standard_session_servicedirs/> so isolated apps never
    // trigger systemd/D-Bus activation of host portals, keyrings, or AT-SPI.
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
    .context("check ChangeProperty AT_SPI_BUS")?;
    conn.sync()?;
    Ok(())
}

pub fn get_xwayland_atspi_bus(x11_display: &str) -> Result<Option<String>> {
    let (conn, screen_num) = x11rb::connect(Some(x11_display))
        .with_context(|| format!("connect to Xwayland {x11_display}"))?;
    let root = conn.setup().roots[screen_num].root;
    let atom = conn
        .intern_atom(true, b"AT_SPI_BUS")?
        .reply()
        .context("intern AT_SPI_BUS")?
        .atom;
    if atom == 0 {
        return Ok(None);
    }
    let reply = conn
        .get_property(false, root, atom, AtomEnum::STRING, 0, 1024)?
        .reply()?;
    if reply.value.is_empty() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&reply.value).into_owned()))
}

#[derive(Default)]
pub struct NotificationStats {
    pub count: AtomicU32,
    pub summaries: std::sync::Mutex<Vec<String>>,
}

pub struct NotificationsService {
    pub stats: Arc<NotificationStats>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationsService {
    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".to_string()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "agent-screen-silent-notifications".to_string(),
            "AgentScreen".to_string(),
            "0.1.0".to_string(),
            "1.2".to_string(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        _body: &str,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id > 0 {
            replaces_id
        } else {
            self.stats.count.fetch_add(1, Ordering::Relaxed) + 1
        };
        self.stats
            .summaries
            .lock()
            .unwrap()
            .push(format!("{app_name}:{summary}"));
        id
    }

    fn close_notification(&self, _id: u32) {}
}

pub async fn start_notifications_stub(
    session_bus_addr: &str,
) -> Result<(zbus::Connection, Arc<NotificationStats>)> {
    let stats = Arc::new(NotificationStats::default());
    let service = NotificationsService {
        stats: stats.clone(),
    };
    let conn = zbus::connection::Builder::address(session_bus_addr)?
        .name("org.freedesktop.Notifications")?
        .serve_at("/org/freedesktop/Notifications", service)?
        .build()
        .await
        .context("serve org.freedesktop.Notifications on isolated session bus")?;
    Ok((conn, stats))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccessibleAppSummary {
    pub bus_name: String,
    pub name: String,
    pub toolkit: String,
    pub node_count: usize,
    pub has_password_role: bool,
    pub sample_nodes: Vec<String>,
}

pub async fn inspect_atspi_bus(bus_address: &str) -> Result<Vec<AccessibleAppSummary>> {
    let conn = zbus::connection::Builder::address(bus_address)?
        .build()
        .await
        .with_context(|| format!("connect to AT-SPI bus {bus_address}"))?;

    let children: Vec<(String, OwnedObjectPath)> = conn
        .call_method(
            Some("org.a11y.atspi.Registry"),
            "/org/a11y/atspi/accessible/root",
            Some("org.a11y.atspi.Accessible"),
            "GetChildren",
            &(),
        )
        .await
        .context("call Registry GetChildren")?
        .body()
        .deserialize()?;

    let mut apps = Vec::new();
    for (bus_name, root_path) in children {
        if bus_name.is_empty() {
            continue;
        }
        let name = get_string_prop(&conn, &bus_name, root_path.as_str(), "Name")
            .await
            .unwrap_or_default();
        let toolkit = get_app_toolkit(&conn, &bus_name, root_path.as_str())
            .await
            .unwrap_or_default();

        let mut node_count = 0usize;
        let mut has_password_role = false;
        let mut sample_nodes = Vec::new();
        walk_accessible(
            &conn,
            &bus_name,
            root_path.as_str(),
            0,
            &mut node_count,
            &mut has_password_role,
            &mut sample_nodes,
        )
        .await;

        apps.push(AccessibleAppSummary {
            bus_name,
            name,
            toolkit,
            node_count,
            has_password_role,
            sample_nodes,
        });
    }
    Ok(apps)
}

async fn get_string_prop(
    conn: &zbus::Connection,
    dest: &str,
    path: &str,
    prop: &str,
) -> Option<String> {
    let msg = conn
        .call_method(
            Some(dest),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.a11y.atspi.Accessible", prop),
        )
        .await
        .ok()?;
    let val: OwnedValue = msg.body().deserialize().ok()?;
    String::try_from(val).ok()
}

async fn get_app_toolkit(conn: &zbus::Connection, dest: &str, path: &str) -> Option<String> {
    let msg = conn
        .call_method(
            Some(dest),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.a11y.atspi.Application", "ToolkitName"),
        )
        .await
        .ok()?;
    let val: OwnedValue = msg.body().deserialize().ok()?;
    String::try_from(val).ok()
}

const ATSPI_ROLE_PASSWORD_TEXT: u32 = 40;

fn walk_accessible<'a>(
    conn: &'a zbus::Connection,
    default_dest: &'a str,
    path: &'a str,
    depth: usize,
    node_count: &'a mut usize,
    has_password_role: &'a mut bool,
    sample_nodes: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if depth > 16 || *node_count >= 250 {
            return;
        }
        *node_count += 1;
        let role_id: u32 = match conn
            .call_method(
                Some(default_dest),
                path,
                Some("org.a11y.atspi.Accessible"),
                "GetRole",
                &(),
            )
            .await
        {
            Ok(m) => m.body().deserialize().unwrap_or(0),
            Err(_) => 0,
        };
        let role_name: String = match conn
            .call_method(
                Some(default_dest),
                path,
                Some("org.a11y.atspi.Accessible"),
                "GetRoleName",
                &(),
            )
            .await
        {
            Ok(m) => m.body().deserialize().unwrap_or_default(),
            Err(_) => String::new(),
        };
        let label = get_string_prop(conn, default_dest, path, "Name")
            .await
            .unwrap_or_default();
        if role_id == ATSPI_ROLE_PASSWORD_TEXT || role_name == "password text" {
            *has_password_role = true;
            sample_nodes.push(format!("{role_id}:{role_name}:{label}"));
        } else if sample_nodes.len() < 12 && (!role_name.is_empty() || !label.is_empty()) {
            sample_nodes.push(format!("{role_id}:{role_name}:{label}"));
        }

        let kids: Vec<(String, OwnedObjectPath)> = match conn
            .call_method(
                Some(default_dest),
                path,
                Some("org.a11y.atspi.Accessible"),
                "GetChildren",
                &(),
            )
            .await
        {
            Ok(m) => m.body().deserialize().unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        for (child_dest, child_path) in kids {
            let dest = if child_dest.is_empty() {
                default_dest
            } else {
                child_dest.as_str()
            };
            walk_accessible(
                conn,
                dest,
                child_path.as_str(),
                depth + 1,
                node_count,
                has_password_role,
                sample_nodes,
            )
            .await;
        }
    })
}
