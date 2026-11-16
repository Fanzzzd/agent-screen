//! Shared `.timeline.jsonl` sidecar writer wrapping
//! `agent_screen_model::recording_playback` so both macOS and Linux recordings
//! emit identical version-1 timelines.

use agent_screen_model::model::shot_scale;
use agent_screen_model::recording_playback::{
    Size, agent_pointer_event, metadata, pointer_event, timeline_end,
};
use serde_json::{Map, Value, json};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub struct FinishedTimeline {
    pub path: PathBuf,
    pub seconds: f64,
    pub actions: u64,
    pub complete: bool,
}

pub struct TimelineWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    logical: Size,
    actions: u64,
    failed: bool,
}

impl TimelineWriter {
    pub fn create(
        path: impl Into<PathBuf>,
        logical_points: (f64, f64),
        video_pixels: (u32, u32),
        codec: &str,
    ) -> Result<Self, String> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("can't create {}: {e}", parent.display()))?;
        }
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            opts.mode(0o600);
        }
        let file = opts
            .open(&path)
            .map_err(|e| format!("can't write {}: {e}", path.display()))?;
        let mut writer = BufWriter::new(file);
        let logical = Size {
            width: logical_points.0,
            height: logical_points.1,
        };
        let video = Size {
            width: video_pixels.0 as f64,
            height: video_pixels.1 as f64,
        };
        let header = metadata(logical, video, shot_scale(logical_points), codec);
        writeln!(writer, "{header}").map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        Ok(Self {
            path,
            writer,
            logical,
            actions: 0,
            failed: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn actions(&self) -> u64 {
        self.actions
    }

    pub fn record_action(&mut self, seconds: f64, mut event: Value) {
        if !seconds.is_finite() || seconds < 0.0 {
            return;
        }
        if event["action"].as_str() == Some("takeoverStart")
            && let Some(hidden) =
                pointer_event(Some(seconds), None, self.logical, self.logical, false)
        {
            self.append(&hidden);
        }
        if let Value::Object(map) = &mut event {
            map.insert("t".into(), json!(seconds));
        }
        self.append(&event);
        self.actions += 1;
    }

    pub fn record_agent_pointer(
        &mut self,
        seconds: f64,
        point: (f64, f64),
        current_size: (f64, f64),
        click: bool,
        active_agent: bool,
    ) {
        if let Some(ev) = agent_pointer_event(
            active_agent,
            || Some(seconds),
            point,
            Size {
                width: current_size.0,
                height: current_size.1,
            },
            self.logical,
            click,
        ) {
            self.append(&ev);
        }
    }

    pub fn finish(mut self, seconds: f64, video_ok: bool) -> FinishedTimeline {
        if self.writer.flush().is_err() {
            self.failed = true;
        }
        let clamped = if seconds.is_finite() && seconds >= 0.0 {
            seconds
        } else {
            0.0
        };
        let complete = !self.failed && video_ok;
        let footer = timeline_end(clamped, complete);
        self.append(&footer);
        let _ = self.writer.flush();
        FinishedTimeline {
            path: self.path,
            seconds: clamped,
            actions: self.actions,
            complete: !self.failed && video_ok,
        }
    }

    fn append(&mut self, value: &Value) {
        if writeln!(self.writer, "{value}").is_err() {
            self.failed = true;
        }
    }
}

/// Extract the sanitized action object recorded for a control-socket method,
/// omitting `displayId`, `sessionKey`, and `null` fields.
pub fn recorded_action_event(method: &str, params: &Map<String, Value>) -> Option<Value> {
    let kept = matches!(
        method,
        "click"
            | "mouseDown"
            | "mouseUp"
            | "move"
            | "drag"
            | "scroll"
            | "type"
            | "key"
            | "holdKey"
            | "elementAction"
            | "launchApp"
            | "placeWindow"
            | "requestHelp"
            | "withdrawHelp"
            | "requestControl"
            | "withdrawControlRequest"
            | "windowAccess"
    );
    if !kept {
        return None;
    }
    let mut event = Map::new();
    event.insert("action".into(), json!(method));
    for (k, v) in params {
        if k != "displayId" && k != "sessionKey" && !v.is_null() {
            event.insert(k.clone(), v.clone());
        }
    }
    Some(Value::Object(event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_screen_model::recording_playback::{Support, Timeline};

    #[test]
    fn timeline_writer_round_trips_through_playback_parser() {
        let path = std::env::temp_dir().join(format!(
            "as-timeline-test-{}-{}.timeline.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut writer = TimelineWriter::create(&path, (1280.0, 800.0), (1280, 800), "avc1")
            .expect("create timeline");
        writer.record_agent_pointer(0.25, (640.0, 400.0), (1280.0, 800.0), true, true);
        let mut params = Map::new();
        params.insert("displayId".into(), json!(1));
        params.insert("x".into(), json!(640));
        params.insert("y".into(), json!(400));
        let action = recorded_action_event("click", &params).unwrap();
        writer.record_action(0.25, action);
        let done = writer.finish(1.2, true);
        assert!(done.complete);
        assert_eq!(done.actions, 1);

        let contents = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let parsed = Timeline::parse(Some(&contents));
        assert_eq!(parsed.support, Support::Ready);
        let sample = parsed.sample(0.30, true).expect("pointer sample");
        assert!((sample.normalized.0 - 0.5).abs() < 1e-6);
        assert!((sample.normalized.1 - 0.5).abs() < 1e-6);
        assert!(sample.click_age.is_some());
    }
}
