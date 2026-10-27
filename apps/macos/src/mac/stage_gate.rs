//! Debug-only hardware claims for a fixture supervised by the stage's peer.
//! No session window records or launched-app sets are changed here.

use super::app::process_state;
use super::{app, cg, claims};
use agent_screen_model::stage_gate::{
    Gate, Identity, ProcessState, may_release_fixture, resolve_identity,
};
use std::{
    collections::HashMap,
    io::Read,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

struct Lease {
    display: u32,
    peer: i32,
    executable: PathBuf,
    gate: Gate,
}
static LEASES: Mutex<Option<HashMap<u64, Lease>>> = Mutex::new(None);
static CLOCK: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
fn now() -> u64 {
    CLOCK.elapsed().as_millis() as u64
}

pub fn prepare(connection: u64, display: u32, peer: i32) -> Result<String, &'static str> {
    let target = std::env::current_exe()
        .map_err(|_| "test bundle path unavailable")?
        .ancestors()
        .nth(4)
        .ok_or("test bundle path unavailable")?
        .to_path_buf();
    let executable = target
        .join("e2e/Field.app/Contents/MacOS/Field")
        .canonicalize()
        .map_err(|_| "Field fixture unavailable")?;
    let mut random = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut random))
        .map_err(|_| "stage token unavailable")?;
    let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let mut leases = LEASES.lock().unwrap();
    let leases = leases.get_or_insert_with(HashMap::new);
    if leases.contains_key(&connection) {
        return Err("stage fixture already prepared");
    }
    leases.insert(
        connection,
        Lease {
            display,
            peer,
            executable,
            gate: Gate::new(token.clone(), now()),
        },
    );
    Ok(token)
}

fn identity(pid: i32, window: u32) -> Option<Identity> {
    let first = process_state(pid);
    if !matches!(first, ProcessState::Running(_)) {
        return None;
    }
    resolve_identity(
        pid,
        window,
        first,
        cg::hidden_window_owner(window),
        process_state(pid),
    )
}

pub fn step(
    connection: u64,
    token: &str,
    action: &str,
    pid: i32,
    window: u32,
) -> Result<(), &'static str> {
    let mut leases = LEASES.lock().unwrap();
    let lease = leases
        .as_mut()
        .and_then(|l| l.get_mut(&connection))
        .ok_or("no stage fixture")?;
    lease.gate.authorize(token, now())?;
    if action == "status" {
        return Ok(());
    }
    let who =
        identity(pid, window).ok_or("hidden window owner unavailable; fixture must stay hidden")?;
    match action {
        "claim" => {
            let proof = app::parent_pid(pid) == Some(lease.peer)
                && app::arguments(pid)
                    .and_then(|a| a.first().cloned())
                    .and_then(|a| PathBuf::from(a).canonicalize().ok())
                    .as_ref()
                    == Some(&lease.executable)
                && app::launched_within(pid, Duration::from_secs(10))
                && cg::window_bounds(window)
                    .is_some_and(|b| cg::display_bounds(lease.display).contains(b.mid()));
            lease.gate.claim(token, now(), who, proof, || {
                claims::stage_claim(window, lease.display)
            })
        }
        "show" => {
            let protected = lease.gate.binding.is_some_and(|(_, generation)| {
                claims::still_stage(window, lease.display, generation)
            });
            lease.gate.show(token, now(), who, protected)
        }
        _ => Err("unknown stage fixture action"),
    }
}

/// Never release a replacement window's claim or a normal Handover's claim.
/// Retain a live fixture's hardware protection until its window disappears.
fn sweep_locked(leases: &mut HashMap<u64, Lease>) {
    leases.retain(|_, lease| {
        let Some((who, generation)) = lease.gate.binding else {
            return true;
        };
        // A failed CG lookup does not prove a live hidden window is gone.
        // Keep protection until the fixture exits, or a different real
        // owner proves that this window id was reused.
        if !may_release_fixture(
            who,
            process_state(who.pid),
            cg::hidden_window_owner(who.window),
        ) {
            return true;
        }
        claims::release_stage(who.window, lease.display, generation).is_err()
    });
}

pub fn cancel(connection: u64) {
    {
        let mut leases = LEASES.lock().unwrap();
        if let Some(leases) = leases.as_mut() {
            if let Some(lease) = leases.get_mut(&connection) {
                lease.gate.cancel();
                if lease.gate.binding.is_none() {
                    leases.remove(&connection);
                }
            }
            sweep_locked(leases);
        }
    }
    // A bounded cleanup task. The fixture exits on stdin/control EOF and
    // has a 60 s lifetime. Keep protection if an OS stall outlives that.
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(65);
        while Instant::now() < deadline {
            let mut guard = LEASES.lock().unwrap();
            let Some(leases) = guard.as_mut() else {
                return;
            };
            sweep_locked(leases);
            if !leases.contains_key(&connection) {
                return;
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(100));
        }
        log::warn!("stage fixture cleanup exceeded its deadline; hardware claim retained");
    });
}

pub fn release(connection: u64, token: &str) -> Result<(), &'static str> {
    let mut guard = LEASES.lock().unwrap();
    let Some(leases) = guard.as_mut() else {
        return Ok(());
    };
    let Some(lease) = leases.get_mut(&connection) else {
        return Ok(());
    };
    if !lease.gate.cleanup_token(token) {
        return Err("stage cleanup token rejected");
    }
    if let Some((who, generation)) = lease.gate.binding {
        if !process_state(who.pid).confirmed_gone(who.started) {
            return Err("fixture exit could not be confirmed before stage release");
        }
        claims::release_stage(who.window, lease.display, generation)
            .map_err(|_| "stage release publication failed")?;
    }
    leases.remove(&connection);
    Ok(())
}

pub fn holds_display(display: u32) -> bool {
    let mut guard = LEASES.lock().unwrap();
    let Some(leases) = guard.as_mut() else {
        return false;
    };
    sweep_locked(leases);
    leases
        .values_mut()
        .filter(|l| l.display == display)
        .any(|lease| {
            lease.gate.cancel();
            let on_source = lease
                .gate
                .binding
                .and_then(|(who, _)| cg::window_bounds(who.window))
                .map(|b| cg::display_bounds(display).contains(b.mid()));
            let state = lease
                .gate
                .binding
                .map(|(who, _)| process_state(who.pid))
                .unwrap_or(ProcessState::Unknown);
            lease.gate.needs_source_display(state, on_source)
        })
}
