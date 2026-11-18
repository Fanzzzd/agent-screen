//! Local recording timelines. Pure media-time sampling, never input replay.

pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_EVENTS: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Ready,
    Missing,
    Legacy,
    Invalid,
    TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub normalized: (f64, f64),
    pub click_age: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Event {
    time: f64,
    at: Option<(f64, f64)>,
    click: bool,
}

pub struct Timeline {
    pub support: Support,
    pub logical: Option<Size>,
    pub video: Option<Size>,
    events: Vec<Event>,
}

impl Timeline {
    pub fn unavailable(support: Support) -> Self {
        Self {
            support,
            logical: None,
            video: None,
            events: Vec::new(),
        }
    }

    pub fn parse(source: Option<&str>) -> Self {
        let Some(source) = source else {
            return Self::unavailable(Support::Missing);
        };
        if source.len() > MAX_BYTES {
            return Self::unavailable(Support::TooLarge);
        }
        let mut result = Self::unavailable(Support::Legacy);
        let mut invalid = false;
        let mut end_required = false;
        let mut ended = false;
        let mut count = 0;
        for line in source.lines().filter(|l| !l.trim().is_empty()) {
            count += 1;
            if count > MAX_EVENTS + 1 {
                return Self::unavailable(Support::TooLarge);
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                invalid = true;
                continue;
            };
            if !value.is_object() {
                invalid = true;
                continue;
            }
            match value["event"].as_str() {
                Some("recording") => {
                    let size = |key: &str| -> Option<Size> {
                        let a = value[key].as_array()?;
                        if a.len() != 2 {
                            return None;
                        }
                        let size = Size {
                            width: a[0].as_f64()?,
                            height: a[1].as_f64()?,
                        };
                        size.valid().then_some(size)
                    };
                    if result.logical.is_some()
                        || value["version"] != 1
                        || value["pointerClock"] != "recorded_duration"
                        || value["coordinateSpace"] != "logical"
                        || value["userPointerRecorded"] == true
                        || !value["shotScale"]
                            .as_f64()
                            .is_some_and(|s| s.is_finite() && s > 0.0)
                    {
                        invalid = true;
                        continue;
                    }
                    result.logical = size("logicalSize");
                    result.video = size("videoSize");
                    if result.logical.is_none() || result.video.is_none() {
                        invalid = true;
                    }
                    result.support = Support::Ready;
                    end_required = value["completeAtEnd"] == true;
                }
                Some("recordingEnd") => {
                    if result.logical.is_none()
                        || ended
                        || value["pointerTimelineComplete"] != true
                        || !value["t"]
                            .as_f64()
                            .is_some_and(|t| t.is_finite() && t >= 0.0)
                    {
                        invalid = true;
                    }
                    ended = true;
                }
                Some("pointer") => {
                    if ended {
                        invalid = true;
                        continue;
                    }
                    let Some(logical) = result.logical else {
                        invalid = true;
                        continue;
                    };
                    let Some(time) = value["t"].as_f64().filter(|t| t.is_finite() && *t >= 0.0)
                    else {
                        invalid = true;
                        continue;
                    };
                    let Some(visible) = value["visible"].as_bool() else {
                        invalid = true;
                        continue;
                    };
                    let at = if visible {
                        if value["actor"] != "agent" {
                            invalid = true;
                            continue;
                        }
                        let point = value["x"].as_f64().zip(value["y"].as_f64());
                        let Some((x, y)) = point.filter(|(x, y)| {
                            x.is_finite()
                                && y.is_finite()
                                && *x >= 0.0
                                && *x < logical.width
                                && *y >= 0.0
                                && *y < logical.height
                        }) else {
                            invalid = true;
                            continue;
                        };
                        Some((x / logical.width, y / logical.height))
                    } else {
                        None
                    };
                    result.events.push(Event {
                        time,
                        at,
                        click: value["click"] == true,
                    });
                }
                _ => {} // Old actions remain readable but are never replayed.
            }
        }
        if invalid || (end_required && !ended) {
            return Self::unavailable(Support::Invalid);
        }
        result.events.sort_by(|a, b| a.time.total_cmp(&b.time));
        result
    }

    pub fn sample(&self, seconds: f64, show: bool) -> Option<Sample> {
        if !show || self.support != Support::Ready || !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        let index = self
            .events
            .partition_point(|event| event.time <= seconds)
            .checked_sub(1)?;
        let event = self.events[index];
        let normalized = event.at?;
        let age = seconds - event.time;
        Some(Sample {
            normalized,
            click_age: (event.click && age < 0.48).then_some(age),
        })
    }
}

impl Size {
    pub fn valid(self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
            && self.width <= 65536.0
            && self.height <= 65536.0
    }
}

pub fn seek_time(seconds: f64, duration: f64) -> f64 {
    if !seconds.is_finite() || !duration.is_finite() || duration <= 0.0 {
        return 0.0;
    }
    seconds.clamp(0.0, duration)
}

pub fn project(sample: Sample, video: Size, viewport: Size) -> Option<(f64, f64)> {
    let (x, y) = sample.normalized;
    if !video.valid()
        || !viewport.valid()
        || !x.is_finite()
        || !y.is_finite()
        || !(0.0..=1.0).contains(&x)
        || !(0.0..=1.0).contains(&y)
    {
        return None;
    }
    let scale = (viewport.width / video.width).min(viewport.height / video.height);
    let (w, h) = (video.width * scale, video.height * scale);
    Some((
        (viewport.width - w) / 2.0 + x * w,
        (viewport.height - h) / 2.0 + y * h,
    ))
}

pub fn sidecar_name(name: &str) -> Option<String> {
    if name.starts_with('.') || name.contains(['/', '\\']) {
        return None;
    }
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && extension.eq_ignore_ascii_case("mp4"))
        .then(|| format!("{stem}.timeline.jsonl"))
}

/// What the player should do for a step of a drag on the progress bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrubCommand {
    Pause,
    Seek(f64),
    Play,
}

/// A drag on the progress bar, as a desktop video player handles one: the
/// video pauses while the bar is held, the playhead follows the pointer
/// instead of the player's last reported time, and playback resumes on
/// release if it was playing when the drag began. The picture keeps its
/// last frame throughout; see `retained_frame`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scrub {
    target: Option<f64>,
    resume: bool,
}

impl Scrub {
    pub fn dragging(&self) -> bool {
        self.target.is_some()
    }

    /// The press that starts a drag, `fraction` along the bar.
    pub fn press(&mut self, fraction: f64, duration: f64, playing: bool) -> Vec<ScrubCommand> {
        let Some(at) = position(fraction, duration) else {
            return Vec::new();
        };
        let mut commands = Vec::new();
        if self.target.is_none() {
            self.resume = playing;
            if playing {
                commands.push(ScrubCommand::Pause);
            }
        }
        self.target = Some(at);
        commands.push(ScrubCommand::Seek(at));
        commands
    }

    /// A move while held. Nothing when the position hasn't changed.
    pub fn drag(&mut self, fraction: f64, duration: f64) -> Option<ScrubCommand> {
        let current = self.target?;
        let at = position(fraction, duration)?;
        (at != current).then(|| {
            self.target = Some(at);
            ScrubCommand::Seek(at)
        })
    }

    /// The release, wherever the pointer is.
    pub fn release(&mut self) -> Vec<ScrubCommand> {
        let Some(at) = self.target.take() else {
            return Vec::new();
        };
        let mut commands = vec![ScrubCommand::Seek(at)];
        if std::mem::take(&mut self.resume) {
            commands.push(ScrubCommand::Play);
        }
        commands
    }

    /// The time to show on the bar and to sample the pointer at.
    pub fn shown(&self, player_seconds: f64) -> f64 {
        self.target.unwrap_or(player_seconds)
    }
}

fn position(fraction: f64, duration: f64) -> Option<f64> {
    (fraction.is_finite() && duration.is_finite() && duration > 0.0)
        .then(|| fraction.clamp(0.0, 1.0) * duration)
}

/// The frame to show: a newly decoded one, otherwise the last. A seek in
/// flight never blanks the picture.
pub fn retained_frame<T>(last: Option<T>, fresh: Option<T>) -> Option<T> {
    fresh.or(last)
}

/// A recording file's display label and local start time, read from the
/// `<label> YYYY-MM-DD HH-MM-SS[-…].mp4` name the recorder writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingName {
    pub label: String,
    /// `YYYY-MM-DD HH:MM:SS`, which also sorts in time order.
    pub started: Option<String>,
}

pub fn recording_name(file: &str) -> RecordingName {
    let stem = file
        .rsplit_once('.')
        .map_or(file, |(stem, _)| stem)
        .to_string();
    let bytes = stem.as_bytes();
    let stamp = |at: usize| {
        let s = bytes.get(at..at + 19)?;
        let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
        let ok = digits.iter().all(|&i| s[i].is_ascii_digit())
            && s[4] == b'-'
            && s[7] == b'-'
            && s[10] == b' '
            && s[13] == b'-'
            && s[16] == b'-';
        ok.then(|| {
            let s = &stem[at..at + 19];
            format!("{} {}:{}:{}", &s[..10], &s[11..13], &s[14..16], &s[17..19])
        })
    };
    // The last stamp wins, so a label that happens to contain a date
    // keeps it.
    for at in (0..bytes.len()).rev() {
        if let Some(started) = stamp(at) {
            let label = stem[..at].trim().to_string();
            return RecordingName {
                label: if label.is_empty() {
                    stem.clone()
                } else {
                    label
                },
                started: Some(started),
            };
        }
    }
    RecordingName {
        label: stem,
        started: None,
    }
}

/// A supported, bounded recording configuration. This selects a codec,
/// not an encoder implementation or a verified quality/size trade-off.
pub fn preferred_codec(available: &[&str]) -> Option<&'static str> {
    ["hvc1", "avc1"]
        .into_iter()
        .find(|codec| available.contains(codec))
}

pub fn metadata(logical: Size, video: Size, shot_scale: f64, codec: &str) -> serde_json::Value {
    serde_json::json!({"event":"recording", "version":1,
        "logicalSize":[logical.width,logical.height], "videoSize":[video.width,video.height],
        "shotScale":shot_scale, "pointerClock":"recorded_duration", "userPointerRecorded":false,
        "coordinateSpace":"logical", "videoScale":[video.width/logical.width,video.height/logical.height],
        "selectedCodec":codec, "hardwareEncoderVerified":false, "completeAtEnd":true})
}

/// The actor gate is supplied at the resolved-point call, not copied at
/// asynchronous recording startup. A blocked actor never reads the clock.
pub fn agent_pointer_event(
    active_agent: bool,
    clock: impl FnOnce() -> Option<f64>,
    point: (f64, f64),
    current: Size,
    logical: Size,
    click: bool,
) -> Option<serde_json::Value> {
    if !active_agent {
        return None;
    }
    pointer_event(clock(), Some(point), current, logical, click)
}

pub fn timeline_end(time: f64, complete: bool) -> serde_json::Value {
    serde_json::json!({"event":"recordingEnd", "t":time, "pointerTimelineComplete":complete})
}

/// Resolved agent-display points, normalized through the current display
/// size into the recording's canonical logical size. No global inputs.
pub fn pointer_event(
    clock: Option<f64>,
    point: Option<(f64, f64)>,
    current: Size,
    logical: Size,
    click: bool,
) -> Option<serde_json::Value> {
    let time = clock.filter(|t| t.is_finite() && *t >= 0.0)?;
    if !current.valid() || !logical.valid() {
        return None;
    }
    if let Some((x, y)) = point {
        if !x.is_finite()
            || !y.is_finite()
            || !(0.0..current.width).contains(&x)
            || !(0.0..current.height).contains(&y)
        {
            return None;
        }
        Some(
            serde_json::json!({"event":"pointer","actor":"agent","visible":true,"t":time,
            "x":x/current.width*logical.width,"y":y/current.height*logical.height,"click":click}),
        )
    } else {
        Some(serde_json::json!({"event":"pointer","actor":"unknown","visible":false,"t":time}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> &'static str {
        r#"{"event":"recording","version":1,"logicalSize":[1280,800],"videoSize":[2560,1600],"shotScale":1,"pointerClock":"recorded_duration","coordinateSpace":"logical"}"#
    }

    #[test]
    fn a_drag_pauses_follows_the_pointer_and_resumes_on_release() {
        use ScrubCommand::*;
        let mut scrub = Scrub::default();
        assert_eq!(scrub.press(0.5, 10.0, true), [Pause, Seek(5.0)]);
        assert!(scrub.dragging());
        // The bar shows the dragged position, not the player's stale time.
        assert_eq!(scrub.shown(1.0), 5.0);
        assert_eq!(scrub.drag(0.8, 10.0), Some(Seek(8.0)));
        assert_eq!(scrub.drag(0.8, 10.0), None);
        // Past either end of the bar clamps to it.
        assert_eq!(scrub.drag(1.7, 10.0), Some(Seek(10.0)));
        assert_eq!(scrub.release(), [Seek(10.0), Play]);
        assert!(!scrub.dragging());
        assert_eq!(scrub.shown(3.0), 3.0);
        assert_eq!(scrub.release(), []);
    }

    #[test]
    fn a_paused_video_stays_paused_and_unknown_lengths_do_nothing() {
        use ScrubCommand::*;
        let mut scrub = Scrub::default();
        assert_eq!(scrub.press(0.25, 8.0, false), [Seek(2.0)]);
        assert_eq!(scrub.release(), [Seek(2.0)]);
        assert_eq!(scrub.press(0.5, 0.0, true), []);
        assert_eq!(scrub.press(f64::NAN, 8.0, true), []);
        assert!(!scrub.dragging());
        assert_eq!(scrub.drag(0.5, 8.0), None);
    }

    #[test]
    fn a_seek_keeps_the_last_frame_until_a_new_one_decodes() {
        assert_eq!(retained_frame(Some("last"), None), Some("last"));
        assert_eq!(retained_frame(Some("last"), Some("new")), Some("new"));
        assert_eq!(retained_frame::<&str>(None, None), None);
    }

    #[test]
    fn recording_names_split_label_and_start_time() {
        assert_eq!(
            recording_name("Fix login 2026-10-27 08-03-09.mp4"),
            RecordingName {
                label: "Fix login".into(),
                started: Some("2026-10-27 08:03:09".into()),
            }
        );
        // The current recorder appends the display id and a unique suffix.
        assert_eq!(
            recording_name("Demo 2026-10-27 14-05-00-7-1793000000000000000.mp4"),
            RecordingName {
                label: "Demo".into(),
                started: Some("2026-10-27 14:05:00".into()),
            }
        );
        // A date inside the label stays part of the label.
        assert_eq!(
            recording_name("Report 2025-01-01 00-00-00 2026-10-27 09-10-11.mp4").label,
            "Report 2025-01-01 00-00-00"
        );
        assert_eq!(
            recording_name("notes.mp4"),
            RecordingName {
                label: "notes".into(),
                started: None,
            }
        );
        assert_eq!(
            recording_name("2026-10-27 09-10-11.mp4").label,
            "2026-10-27 09-10-11"
        );
    }

    #[test]
    fn old_and_missing_sidecars_do_not_invent_pointer_coordinates() {
        assert_eq!(Timeline::parse(None).support, Support::Missing);
        let old = Timeline::parse(Some(r#"{"action":"click","x":500,"y":200,"t":1}"#));
        assert_eq!(old.support, Support::Legacy);
        assert!(old.sample(2.0, true).is_none());
    }

    #[test]
    fn media_time_seek_is_deterministic_and_takeover_hides_unknown_input() {
        let text = format!(
            "{}\n{}\n{}\n{}",
            header(),
            r#"{"event":"pointer","actor":"agent","t":1,"x":640,"y":400,"click":true,"visible":true}"#,
            r#"{"event":"pointer","actor":"unknown","t":2,"visible":false}"#,
            r#"{"event":"pointer","actor":"agent","t":4,"x":320,"y":200,"visible":true}"#
        );
        let timeline = Timeline::parse(Some(&text));
        assert_eq!(timeline.support, Support::Ready);
        assert!(timeline.sample(0.5, true).is_none());
        let at = timeline.sample(1.2, true).unwrap();
        assert_eq!(at.normalized, (0.5, 0.5));
        assert!((at.click_age.unwrap() - 0.2).abs() < 1e-6);
        assert!(timeline.sample(2.5, true).is_none());
        assert_eq!(timeline.sample(4.5, true).unwrap().normalized, (0.25, 0.25));
        assert_eq!(timeline.sample(1.2, true), Some(at));
        assert!(timeline.sample(1.2, false).is_none());
        assert_eq!(timeline.sample(1.2, true), Some(at));
    }

    #[test]
    fn coordinates_use_logical_size_and_project_into_letterboxed_video() {
        let sample = Sample {
            normalized: (0.25, 0.5),
            click_age: None,
        };
        assert_eq!(
            project(
                sample,
                Size {
                    width: 2560.0,
                    height: 1600.0
                },
                Size {
                    width: 400.0,
                    height: 400.0
                }
            ),
            Some((100.0, 200.0))
        );
        assert!(
            project(
                sample,
                Size {
                    width: 0.0,
                    height: 1600.0
                },
                Size {
                    width: 400.0,
                    height: 400.0
                }
            )
            .is_none()
        );
    }

    #[test]
    fn malformed_unknown_version_and_oversize_timelines_disable_only_pointer() {
        assert_eq!(
            Timeline::parse(Some(&format!("{}\nnot-json", header()))).support,
            Support::Invalid
        );
        assert_eq!(
            Timeline::parse(Some(&header().replace("\"version\":1", "\"version\":99"))).support,
            Support::Invalid
        );
        assert_eq!(
            Timeline::parse(Some(&" ".repeat(MAX_BYTES + 1))).support,
            Support::TooLarge
        );
    }

    #[test]
    fn event_limits_bad_records_and_coordinate_versions_fail_closed() {
        let source = format!("{}\n{}", header(), "{}\n".repeat(MAX_EVENTS + 1));
        assert_eq!(Timeline::parse(Some(&source)).support, Support::TooLarge);
        for suffix in [
            "[]",
            "null",
            "{\"event\":\"pointer\",\"t\":1}",
            "{\"event\":",
        ] {
            assert_eq!(
                Timeline::parse(Some(&format!("{}\n{}", header(), suffix))).support,
                Support::Invalid
            );
        }
        let incompatible = header().replace("\"logical\"", "\"screenshot-pixels\"");
        assert_eq!(
            Timeline::parse(Some(&incompatible)).support,
            Support::Invalid
        );
    }

    #[test]
    fn duplicate_media_times_keep_file_order_and_unknown_actions_are_not_replayed() {
        let text = format!(
            "{}\n{}\n{}\n{}\n{}",
            header(),
            r#"{"event":"pointer","actor":"agent","t":2,"x":640,"y":400,"visible":true}"#,
            r#"{"event":"pointer","actor":"agent","t":1,"x":320,"y":200,"visible":true}"#,
            r#"{"event":"pointer","actor":"unknown","t":2,"visible":false}"#,
            r#"{"url":"https://example.invalid/video.mp4","action":"click","t":1.5,"x":999,"y":999}"#
        );
        let timeline = Timeline::parse(Some(&text));
        assert_eq!(timeline.support, Support::Ready);
        assert_eq!(
            timeline.sample(1.75, true).unwrap().normalized,
            (0.25, 0.25)
        );
        assert!(timeline.sample(2.0, true).is_none());
        assert_eq!(timeline.sample(1.75, true), timeline.sample(1.75, true));
        assert!(timeline.sample(f64::NAN, true).is_none());
    }

    #[test]
    fn nonfinite_times_and_off_display_points_are_not_replayed() {
        let text = format!(
            "{}\n{}",
            header(),
            r#"{"event":"pointer","actor":"agent","t":1,"x":-1,"y":10,"visible":true}"#
        );
        assert_eq!(Timeline::parse(Some(&text)).support, Support::Invalid);
        assert_eq!(seek_time(f64::NAN, 10.0), 0.0);
        assert_eq!(seek_time(20.0, 10.0), 10.0);
        assert_eq!(seek_time(-1.0, 10.0), 0.0);
        assert_eq!(
            sidecar_name("Session.MP4").as_deref(),
            Some("Session.timeline.jsonl")
        );
        assert!(sidecar_name("../Session.mp4").is_none());
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn new_capture_requires_a_complete_sidecar_but_video_is_independent() {
        let size = Size {
            width: 1280.0,
            height: 800.0,
        };
        let header = metadata(size, size, 1.0, "hvc1").to_string();
        assert_eq!(Timeline::parse(Some(&header)).support, Support::Invalid);
        assert_eq!(
            Timeline::parse(Some(&format!("{}\n{}", header, timeline_end(2.0, false)))).support,
            Support::Invalid
        );
        assert_eq!(
            Timeline::parse(Some(&format!("{}\n{}", header, timeline_end(2.0, true)))).support,
            Support::Ready
        );
    }

    #[test]
    fn current_actor_gate_cannot_go_stale_during_recording_start() {
        let size = Size {
            width: 1280.0,
            height: 800.0,
        };
        let takeover = std::cell::Cell::new(true);
        // Control can return while asynchronous recording startup runs.
        takeover.set(false);
        assert!(
            agent_pointer_event(
                !takeover.get(),
                || Some(1.0),
                (640.0, 400.0),
                size,
                size,
                true
            )
            .is_some()
        );
        takeover.set(true);
        assert!(
            agent_pointer_event(
                !takeover.get(),
                || panic!("Takeover read the recording clock"),
                (640.0, 400.0),
                size,
                size,
                true
            )
            .is_none()
        );
    }

    #[test]
    fn codec_policy_prefers_available_hevc_and_falls_back_to_h264_only() {
        assert_eq!(preferred_codec(&["avc1", "hvc1"]), Some("hvc1"));
        assert_eq!(preferred_codec(&["avc1"]), Some("avc1"));
        assert_eq!(preferred_codec(&["jpeg", "av01"]), None);
    }

    #[test]
    fn resolved_metadata_round_trips_without_recording_text_or_unstarted_events() {
        let logical = Size {
            width: 1280.0,
            height: 800.0,
        };
        let current = Size {
            width: 640.0,
            height: 400.0,
        };
        assert!(pointer_event(None, Some((320.0, 200.0)), current, logical, true).is_none());
        assert!(pointer_event(Some(1.0), Some((-1.0, 200.0)), current, logical, true).is_none());
        let point =
            pointer_event(Some(1.25), Some((320.0, 200.0)), current, logical, true).unwrap();
        let mut keys = point
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, ["actor", "click", "event", "t", "visible", "x", "y"]);
        let source = format!(
            "{}\n{}\n{}\n{}",
            metadata(
                logical,
                Size {
                    width: 2560.0,
                    height: 1600.0
                },
                1.0,
                "hvc1"
            ),
            point,
            pointer_event(Some(2.0), None, current, logical, false).unwrap(),
            timeline_end(3.0, true)
        );
        let timeline = Timeline::parse(Some(&source));
        assert_eq!(timeline.sample(1.25, true).unwrap().normalized, (0.5, 0.5));
        assert!(timeline.sample(2.5, true).is_none());
    }
}
