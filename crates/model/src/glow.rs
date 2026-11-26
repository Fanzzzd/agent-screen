//! The Red Glow: the red light along the edges of each of the user's
//! screens while an Agent Session has, or may at any moment take, their
//! pointer or keyboard. Which state it is in, from the facts of the Input
//! Queue, a Keyboard Borrow and a front hold, and the numbers of its look,
//! which the app turns into layers. No OS calls.
//!
//! The look: a solid rim on the glass's edge, a white hairline just inside
//! it, and three soft red bands reaching inward, narrow to wide. Each band
//! is the blurred shadow of the outline stroked at its width, kept on the
//! glass, so its inner half fades into the screen with no hard edge.
//! `CONTRACT` is the specification of the result; the bands are tuned to
//! meet it, so a retune is an edit of these constants.

/// What the glow shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Glow {
    /// No window on screen.
    Off,
    /// An Input Grant is open with no loan running, or cleanup restores the
    /// pointer and front: dimmer, steady.
    Armed,
    /// Input goes out: full strength, breathing.
    Live,
}

impl Glow {
    pub fn name(self) -> &'static str {
        match self {
            Glow::Off => "off",
            Glow::Armed => "armed",
            Glow::Live => "live",
        }
    }
}

/// What the glow's state follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// An Input Grant is open.
    pub grant_open: bool,
    /// An Input Loan runs or is stopping.
    pub loan: bool,
    /// The grant has ended and cleanup restores the pointer and front.
    pub restoring: bool,
    /// A Keyboard Borrow runs, on any display.
    pub keyboard_borrow: bool,
    /// A front hold has the front.
    pub front_hold: bool,
    /// A preview `debugShow` asked for.
    pub preview: Option<Glow>,
}

/// The state the facts call for. The real state wins over a preview: the
/// result is the stronger of the two.
pub fn state(f: &Facts) -> Glow {
    let real = if f.loan || f.keyboard_borrow || f.front_hold {
        Glow::Live
    } else if f.grant_open || f.restoring {
        Glow::Armed
    } else {
        Glow::Off
    };
    real.max(f.preview.unwrap_or(Glow::Off))
}

/// The screen size, in points, the bands are drawn for at scale 1: a 14-inch
/// MacBook Pro's 1512 by 982.
pub const BASE_PT: f64 = 982.0;

/// How much the bands grow on a screen `width` by `height` points: its
/// shorter side over `BASE_PT`, within 0.85 and 1.75.
pub fn scale(width: f64, height: f64) -> f64 {
    (width.min(height) / BASE_PT).clamp(0.85, 1.75)
}

/// One soft band: the glass's outline stroked `width` wide casts a red
/// shadow blurred by `radius`, at `opacity`, and the glass keeps its inner
/// half. Points at scale 1; a `scaled` band grows with `scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub width: f64,
    pub radius: f64,
    pub opacity: f64,
    pub scaled: bool,
}

impl Band {
    /// The stroke's width and the blur on a screen of scale `u`.
    pub fn at(self, u: f64) -> (f64, f64) {
        let u = if self.scaled { u } else { 1.0 };
        (self.width * u, self.radius * u)
    }
}

/// The bands, narrow to wide: the core along the rim, the halo, and the
/// wash that tints the screen's perimeter.
pub const BANDS: [Band; 3] = [
    // core
    Band {
        width: 10.0,
        radius: 8.0,
        opacity: 0.70,
        scaled: false,
    },
    // halo
    Band {
        width: 28.0,
        radius: 16.0,
        opacity: 0.65,
        scaled: true,
    },
    // wash
    Band {
        width: 70.0,
        radius: 44.0,
        opacity: 0.45,
        scaled: true,
    },
];

/// The solid red rim's width, from the glass's edge inward, in points.
pub const RIM_PT: f64 = 3.0;

/// The white hairline's width, just inside the rim, and its alpha. It keeps
/// the rim readable on a red or dark wallpaper.
pub const HAIRLINE_PT: f64 = 1.0;
pub const HAIRLINE_ALPHA: f64 = 0.55;

/// The width each rim layer strokes the outline at: twice what shows, as
/// the glass keeps only the inner half.
pub fn rim_stroke() -> f64 {
    2.0 * RIM_PT
}

pub fn hairline_stroke() -> f64 {
    2.0 * (RIM_PT + HAIRLINE_PT)
}

/// Display P3 red, the same in light and dark appearance: the wallpaper
/// decides the backdrop.
pub const RED_P3: (f64, f64, f64) = (1.00, 0.16, 0.12);

/// The opacities of a state: the bands' (`pulse`) and the rims'.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub pulse: f64,
    pub rims: f64,
}

pub const LIVE: Look = Look {
    pulse: 1.0,
    rims: 1.0,
};

pub const ARMED: Look = Look {
    pulse: 0.55,
    rims: 0.8,
};

/// The look of a state; None for Off.
pub fn look(glow: Glow) -> Option<Look> {
    match glow {
        Glow::Off => None,
        Glow::Armed => Some(ARMED),
        Glow::Live => Some(LIVE),
    }
}

/// Live breathes: the bands' opacity goes from `LIVE.pulse` down to
/// `BREATH_LOW` and back over `BREATH_S` seconds, unless the user asked
/// macOS to reduce motion.
pub const BREATH_LOW: f64 = 0.70;
pub const BREATH_S: f64 = 1.8;

/// Appearing, both opacities rise from 0 over `APPEAR_S`; Armed to Live and
/// back takes `CHANGE_S`. Off is at once. Real input waits for the window,
/// never for the fade.
pub const APPEAR_S: f64 = 0.12;
pub const CHANGE_S: f64 = 0.15;

/// A preview ends by itself after this long, so a script that dies cannot
/// leave the glow on.
pub const PREVIEW_MS: u64 = 10_000;

/// The specification of the result: the alpha of the glow window alone,
/// Live at the top of its breath, `d` points in from the glass's edge on
/// the perpendicular at the middle of an edge, at scale 1: (d, alpha,
/// tolerance). The first row is the rim; the last two are upper bounds,
/// alpha 0 plus the tolerance. The last row's distance is `FAR` of the
/// screen's shorter side, and beyond.
pub const CONTRACT: [(f64, f64, f64); 6] = [
    (1.0, 1.0, 0.05),
    (8.0, 0.62, 0.08),
    (24.0, 0.36, 0.06),
    (64.0, 0.11, 0.04),
    (120.0, 0.0, 0.03),
    (FAR * BASE_PT, 0.0, 0.01),
];

/// Past this share of the screen's shorter side, nothing changes.
pub const FAR: f64 = 0.22;

/// How far in, in points, to read row `d` of `CONTRACT` on a screen
/// `width` by `height`: `d` times the scale, and the last row at `FAR` of
/// the shorter side.
pub fn distance(d: f64, width: f64, height: f64) -> f64 {
    if d >= FAR * BASE_PT {
        FAR * width.min(height)
    } else {
        d * scale(width, height)
    }
}

/// The alpha a row of `CONTRACT` asks for, and its tolerance, in `glow`.
/// Armed scales each row from d = 8 on by its bands' opacity, tolerance
/// too. None where the row says nothing: Off, and the rim in Armed.
pub fn expected(row: (f64, f64, f64), glow: Glow) -> Option<(f64, f64)> {
    let (d, alpha, tolerance) = row;
    match glow {
        Glow::Off => None,
        Glow::Live => Some((alpha, tolerance)),
        Glow::Armed if d < 8.0 => None,
        Glow::Armed => Some((alpha * ARMED.pulse, tolerance * ARMED.pulse)),
    }
}

/// Whether `alpha` read at `row` meets the contract in `glow`.
pub fn meets(row: (f64, f64, f64), alpha: f64, glow: Glow) -> bool {
    expected(row, glow).is_none_or(|(want, tolerance)| (alpha - want).abs() <= tolerance + 1e-9)
}

/// Under the notch's bottom edge and on the 45° line into each top corner,
/// the alpha at d = 8 is within this of the edge's: the glow follows the
/// glass there.
pub const FOLLOW: f64 = 0.12;

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts::default()
    }

    #[test]
    fn nothing_is_off() {
        assert_eq!(state(&facts()), Glow::Off);
    }

    #[test]
    fn input_going_out_is_live() {
        for f in [
            Facts {
                grant_open: true,
                loan: true,
                ..facts()
            },
            Facts {
                keyboard_borrow: true,
                ..facts()
            },
            Facts {
                front_hold: true,
                ..facts()
            },
            // A loan stopping after the grant ended.
            Facts {
                loan: true,
                restoring: true,
                ..facts()
            },
        ] {
            assert_eq!(state(&f), Glow::Live, "{f:?}");
        }
    }

    #[test]
    fn an_open_grant_or_cleanup_is_armed() {
        for f in [
            Facts {
                grant_open: true,
                ..facts()
            },
            Facts {
                restoring: true,
                ..facts()
            },
        ] {
            assert_eq!(state(&f), Glow::Armed, "{f:?}");
        }
    }

    #[test]
    fn the_real_state_beats_a_weaker_preview() {
        let f = Facts {
            loan: true,
            grant_open: true,
            preview: Some(Glow::Armed),
            ..facts()
        };
        assert_eq!(state(&f), Glow::Live);
        let f = Facts {
            grant_open: true,
            preview: Some(Glow::Off),
            ..facts()
        };
        assert_eq!(state(&f), Glow::Armed);
    }

    #[test]
    fn a_stronger_preview_shows() {
        let f = Facts {
            grant_open: true,
            preview: Some(Glow::Live),
            ..facts()
        };
        assert_eq!(state(&f), Glow::Live);
    }

    #[test]
    fn a_preview_alone_shows() {
        for glow in [Glow::Armed, Glow::Live] {
            let f = Facts {
                preview: Some(glow),
                ..facts()
            };
            assert_eq!(state(&f), glow);
        }
    }

    #[test]
    fn scale_follows_the_shorter_side_within_bounds() {
        assert!((scale(1512.0, 982.0) - 1.0).abs() < 1e-12);
        assert!((scale(1728.0, 1117.0) - 1117.0 / 982.0).abs() < 1e-12);
        assert_eq!(scale(1024.0, 640.0), 0.85);
        assert_eq!(scale(6016.0, 3384.0), 1.75);
    }

    #[test]
    fn bands_go_narrow_to_wide() {
        for pair in BANDS.windows(2) {
            assert!(pair[0].width < pair[1].width, "{pair:?}");
            assert!(pair[0].radius < pair[1].radius, "{pair:?}");
        }
        for band in BANDS {
            assert!(band.opacity > 0.0 && band.opacity <= 1.0, "{band:?}");
        }
    }

    #[test]
    fn the_rim_is_narrower_than_the_hairlines_stroke() {
        assert!(rim_stroke() < hairline_stroke());
        const {
            assert!(HAIRLINE_ALPHA > 0.0 && HAIRLINE_ALPHA < 1.0);
        }
    }

    #[test]
    fn armed_is_dimmer_and_live_breathes_below_its_top() {
        const {
            assert!(ARMED.pulse < LIVE.pulse && ARMED.rims < LIVE.rims);
        }
        const {
            assert!(BREATH_LOW < LIVE.pulse && BREATH_LOW > ARMED.pulse);
        }
        assert_eq!(look(Glow::Off), None);
    }

    #[test]
    fn the_contract_reads_farther_in_on_bigger_screens() {
        let (w, h) = (1728.0, 1117.0);
        let u = scale(w, h);
        assert!((distance(8.0, w, h) - 8.0 * u).abs() < 1e-12);
        assert!((distance(CONTRACT[5].0, w, h) - FAR * h).abs() < 1e-12);
        assert!((distance(CONTRACT[5].0, 1512.0, 982.0) - CONTRACT[5].0).abs() < 1e-9);
        // Clamped, the far row still reads at a fifth of the screen.
        assert!((distance(CONTRACT[5].0, 1024.0, 640.0) - FAR * 640.0).abs() < 1e-12);
    }

    #[test]
    fn the_contract_falls_off_inward() {
        for pair in CONTRACT.windows(2) {
            assert!(pair[0].0 < pair[1].0 && pair[0].1 >= pair[1].1, "{pair:?}");
        }
    }

    #[test]
    fn armed_scales_the_contract_from_eight_points_on() {
        assert_eq!(expected(CONTRACT[0], Glow::Armed), None);
        let (want, tolerance) = expected(CONTRACT[1], Glow::Armed).unwrap();
        assert!((want - 0.62 * 0.55).abs() < 1e-12 && (tolerance - 0.08 * 0.55).abs() < 1e-12);
        assert!(meets(CONTRACT[1], 0.62 * 0.55, Glow::Armed));
        assert!(!meets(CONTRACT[1], 0.62, Glow::Armed));
        assert!(meets(CONTRACT[0], 0.97, Glow::Live));
        assert!(!meets(CONTRACT[0], 0.9, Glow::Live));
        assert!(meets(CONTRACT[4], 0.03, Glow::Live));
        assert!(!meets(CONTRACT[5], 0.02, Glow::Live));
    }
}
