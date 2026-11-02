//! Where the menu-bar panel opens (pure): under the menu-bar icon on macOS, above / beside the
//! tray icon on Windows / Linux, clamped into the work area of the screen the icon is on.
//!
//! All rectangles are in **one** coordinate space with y growing downwards: macOS points (each
//! monitor's physical rect divided by its own scale factor, since macOS mixes scales per monitor),
//! physical pixels on Windows / Linux (their virtual desktop is one physical space).

/// The panel's width (points / logical pixels).
pub const WIDTH: f64 = 360.0;
/// Content-fitted height: never smaller / larger than this; beyond it the list scrolls.
pub const MIN_HEIGHT: f64 = 220.0;
pub const MAX_HEIGHT: f64 = 560.0;
/// Space between the icon (or the bar) and the panel.
pub const GAP: f64 = 6.0;
/// Space kept to the other edges of the work area.
pub const MARGIN: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, (px, py): (f64, f64)) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
    /// Distance from a point to this rectangle (0 inside).
    fn distance(&self, (px, py): (f64, f64)) -> f64 {
        let dx = (self.x - px).max(px - self.right()).max(0.0);
        let dy = (self.y - py).max(py - self.bottom()).max(0.0);
        (dx * dx + dy * dy).sqrt()
    }
    /// Divides every value (physical → points on macOS).
    pub fn scaled(&self, factor: f64) -> Self {
        let f = if factor > 0.0 { factor } else { 1.0 };
        Self::new(self.x / f, self.y / f, self.w / f, self.h / f)
    }
}

/// One monitor: its whole area and the part not covered by the menu bar / Dock / taskbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub bounds: Rect,
    pub work: Rect,
}

/// The screen edge the menu bar / taskbar (and so the icon) is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    /// Where the bar is when there is no icon rectangle (opened from the menu, Linux): the menu
    /// bar on macOS, the taskbar on Windows, a top panel on most Linux desktops.
    pub fn default_for(os: &str) -> Self {
        match os {
            "windows" => Edge::Bottom,
            _ => Edge::Top,
        }
    }
}

/// The screen showing `point` (the icon's centre); else the nearest one (an icon reported just
/// outside every screen). `None` only without screens.
pub fn pick_screen(screens: &[Screen], point: (f64, f64)) -> Option<usize> {
    screens.iter().position(|s| s.bounds.contains(point)).or_else(|| {
        screens
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.bounds.distance(point).total_cmp(&b.bounds.distance(point)))
            .map(|(i, _)| i)
    })
}

/// The bar the icon sits in. Normally the icon lies outside the work area, on the bar's side
/// (the work area alone can't tell: macOS takes the menu bar *and* the Dock out of it). An icon
/// inside the work area (auto-hidden bar, some Linux panels): top / bottom when it is within a
/// bar's height of them, else the nearest edge.
pub fn bar_edge(screen: &Screen, icon: Option<Rect>, fallback: Edge) -> Edge {
    let Some(icon) = icon else { return fallback };
    let (cx, cy) = icon.center();
    let (w, b) = (screen.work, screen.bounds);
    if cy < w.y {
        return Edge::Top;
    }
    if cy >= w.bottom() {
        return Edge::Bottom;
    }
    if cx < w.x {
        return Edge::Left;
    }
    if cx >= w.right() {
        return Edge::Right;
    }
    let (top, bottom) = (cy - b.y, b.bottom() - cy);
    if top.min(bottom) <= BAR_REACH {
        return if top <= bottom { Edge::Top } else { Edge::Bottom };
    }
    let (left, right) = (cx - b.x, b.right() - cx);
    [
        (Edge::Top, top),
        (Edge::Bottom, bottom),
        (Edge::Left, left),
        (Edge::Right, right),
    ]
    .into_iter()
    .min_by(|a, b| a.1.total_cmp(&b.1))
    .map(|(edge, _)| edge)
    .unwrap_or(fallback)
}

/// How far from the top / bottom an icon may be and still count as being in a horizontal bar.
const BAR_REACH: f64 = 64.0;

/// The panel height for the page's content height.
pub fn clamp_height(content: f64) -> f64 {
    if content.is_finite() {
        content.clamp(MIN_HEIGHT, MAX_HEIGHT)
    } else {
        MIN_HEIGHT
    }
}

fn clamp(value: f64, lo: f64, hi: f64) -> f64 {
    if hi < lo {
        lo
    } else {
        value.clamp(lo, hi)
    }
}

/// Top-left corner of a `w` × `h` panel: next to the icon on the bar's side, centred on the icon
/// along the bar, inside the work area (the bar's side gets [`GAP`], the others [`MARGIN`]).
/// Without an icon: the work area's corner at the end of the bar (where the clock is).
pub fn place(screen: &Screen, icon: Option<Rect>, (w, h): (f64, f64), fallback: Edge) -> (f64, f64) {
    let work = screen.work;
    let edge = bar_edge(screen, icon, fallback);
    // the near edge (towards the bar) keeps GAP, the far edges MARGIN
    let x_range = |near_left: bool, near_right: bool| {
        let lo = work.x + if near_left { GAP } else { MARGIN };
        let hi = work.right() - w - if near_right { GAP } else { MARGIN };
        (lo, hi)
    };
    let y_range = |near_top: bool, near_bottom: bool| {
        let lo = work.y + if near_top { GAP } else { MARGIN };
        let hi = work.bottom() - h - if near_bottom { GAP } else { MARGIN };
        (lo, hi)
    };
    match edge {
        Edge::Top => {
            let (x, y) = match icon {
                Some(i) => (i.center().0 - w / 2.0, i.bottom().max(work.y) + GAP),
                None => (work.right() - w - MARGIN, work.y + GAP),
            };
            let (xl, xh) = x_range(false, false);
            let (yl, yh) = y_range(true, false);
            (clamp(x, xl, xh), clamp(y, yl, yh))
        }
        Edge::Bottom => {
            let (x, y) = match icon {
                Some(i) => (i.center().0 - w / 2.0, i.y.min(work.bottom()) - GAP - h),
                None => (work.right() - w - MARGIN, work.bottom() - GAP - h),
            };
            let (xl, xh) = x_range(false, false);
            let (yl, yh) = y_range(false, true);
            (clamp(x, xl, xh), clamp(y, yl, yh))
        }
        Edge::Left => {
            let (x, y) = match icon {
                Some(i) => (i.right().max(work.x) + GAP, i.center().1 - h / 2.0),
                None => (work.x + GAP, work.bottom() - h - MARGIN),
            };
            let (xl, xh) = x_range(true, false);
            let (yl, yh) = y_range(false, false);
            (clamp(x, xl, xh), clamp(y, yl, yh))
        }
        Edge::Right => {
            let (x, y) = match icon {
                Some(i) => (i.x.min(work.right()) - GAP - w, i.center().1 - h / 2.0),
                None => (work.right() - w - GAP, work.bottom() - h - MARGIN),
            };
            let (xl, xh) = x_range(false, true);
            let (yl, yh) = y_range(false, false);
            (clamp(x, xl, xh), clamp(y, yl, yh))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1512 × 982 pt MacBook screen: menu bar 33 pt, Dock 70 pt at the bottom.
    fn macbook() -> Screen {
        Screen {
            bounds: Rect::new(0.0, 0.0, 1512.0, 982.0),
            work: Rect::new(0.0, 33.0, 1512.0, 879.0),
        }
    }

    /// A 1920 × 1080 px Windows screen with the taskbar (48 px) at the bottom.
    fn windows_bottom() -> Screen {
        Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work: Rect::new(0.0, 0.0, 1920.0, 1032.0),
        }
    }

    const SIZE: (f64, f64) = (WIDTH, 400.0);

    #[test]
    fn opens_right_under_the_menu_bar_icon_centred_on_it() {
        let icon = Rect::new(1200.0, 4.0, 26.0, 25.0);
        let s = macbook();
        assert_eq!(
            bar_edge(&s, Some(icon), Edge::Top),
            Edge::Top,
            "the Dock must not count"
        );
        let (x, y) = place(&s, Some(icon), SIZE, Edge::Top);
        assert_eq!(x, 1213.0 - WIDTH / 2.0);
        assert_eq!(y, 33.0 + GAP, "under the menu bar, not the icon's 29 pt");
    }

    #[test]
    fn an_icon_near_the_right_edge_keeps_the_panel_on_screen() {
        let icon = Rect::new(1480.0, 4.0, 26.0, 25.0);
        let (x, _) = place(&macbook(), Some(icon), SIZE, Edge::Top);
        assert_eq!(x, 1512.0 - WIDTH - MARGIN);
        // and at the left edge
        let icon = Rect::new(2.0, 4.0, 26.0, 25.0);
        assert_eq!(place(&macbook(), Some(icon), SIZE, Edge::Top).0, MARGIN);
        // the very last icon of the menu bar is still in the menu bar, not a bar on the right
        let icon = Rect::new(1500.0, 4.0, 12.0, 25.0);
        assert_eq!(bar_edge(&macbook(), Some(icon), Edge::Top), Edge::Top);
    }

    #[test]
    fn auto_hidden_menu_bar_goes_under_the_icon() {
        let s = Screen {
            bounds: Rect::new(0.0, 0.0, 1512.0, 982.0),
            work: Rect::new(0.0, 0.0, 1512.0, 982.0),
        };
        let icon = Rect::new(1200.0, 0.0, 26.0, 24.0);
        assert_eq!(place(&s, Some(icon), SIZE, Edge::Top).1, 24.0 + GAP);
    }

    #[test]
    fn windows_taskbar_at_the_bottom_opens_above_the_icon() {
        let icon = Rect::new(1700.0, 1040.0, 32.0, 40.0);
        let s = windows_bottom();
        assert_eq!(bar_edge(&s, Some(icon), Edge::Bottom), Edge::Bottom);
        let (x, y) = place(&s, Some(icon), SIZE, Edge::Bottom);
        assert_eq!(x, 1716.0 - WIDTH / 2.0);
        assert_eq!(y, 1032.0 - GAP - 400.0);
        // the clock corner: clamped to the right edge
        let icon = Rect::new(1880.0, 1040.0, 32.0, 40.0);
        assert_eq!(place(&s, Some(icon), SIZE, Edge::Bottom).0, 1920.0 - WIDTH - MARGIN);
    }

    #[test]
    fn taskbar_on_the_left_or_right_opens_beside_it() {
        let left = Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work: Rect::new(62.0, 0.0, 1858.0, 1080.0),
        };
        let icon = Rect::new(15.0, 980.0, 32.0, 32.0);
        assert_eq!(bar_edge(&left, Some(icon), Edge::Bottom), Edge::Left);
        let (x, y) = place(&left, Some(icon), SIZE, Edge::Bottom);
        assert_eq!(x, 62.0 + GAP);
        assert_eq!(y, 1080.0 - 400.0 - MARGIN, "centred on the icon, clamped to the bottom");

        let right = Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work: Rect::new(0.0, 0.0, 1858.0, 1080.0),
        };
        let icon = Rect::new(1873.0, 500.0, 32.0, 32.0);
        assert_eq!(bar_edge(&right, Some(icon), Edge::Bottom), Edge::Right);
        let (x, y) = place(&right, Some(icon), SIZE, Edge::Bottom);
        assert_eq!(x, 1858.0 - GAP - WIDTH);
        assert_eq!(y, 516.0 - 200.0);
    }

    #[test]
    fn a_second_monitor_left_of_and_above_the_main_one() {
        // main 2560 × 1440 at 0,0; a second screen to the left at negative x, a third above
        let screens = [
            Screen {
                bounds: Rect::new(0.0, 0.0, 2560.0, 1440.0),
                work: Rect::new(0.0, 25.0, 2560.0, 1415.0),
            },
            Screen {
                bounds: Rect::new(-1920.0, 200.0, 1920.0, 1080.0),
                work: Rect::new(-1920.0, 225.0, 1920.0, 1055.0),
            },
            Screen {
                bounds: Rect::new(0.0, -1080.0, 1920.0, 1080.0),
                work: Rect::new(0.0, -1055.0, 1920.0, 1055.0),
            },
        ];
        let icon = Rect::new(-300.0, 202.0, 26.0, 22.0);
        let i = pick_screen(&screens, icon.center()).unwrap();
        assert_eq!(i, 1);
        let (x, y) = place(&screens[i], Some(icon), SIZE, Edge::Top);
        assert_eq!(x, -287.0 - WIDTH / 2.0);
        assert_eq!(y, 225.0 + GAP);
        assert!(x + WIDTH <= -MARGIN, "stays on the left screen");

        let icon = Rect::new(1890.0, -1078.0, 26.0, 22.0);
        let i = pick_screen(&screens, icon.center()).unwrap();
        assert_eq!(i, 2);
        let (x, y) = place(&screens[i], Some(icon), SIZE, Edge::Top);
        assert_eq!(x, 1920.0 - WIDTH - MARGIN);
        assert_eq!(y, -1055.0 + GAP);
    }

    #[test]
    fn an_icon_just_outside_every_screen_uses_the_nearest() {
        let screens = [macbook()];
        assert_eq!(pick_screen(&screens, (1600.0, 10.0)), Some(0));
        assert_eq!(pick_screen(&[], (0.0, 0.0)), None);
    }

    #[test]
    fn without_an_icon_rect_it_opens_in_the_bars_corner() {
        assert_eq!(Edge::default_for("macos"), Edge::Top);
        assert_eq!(Edge::default_for("linux"), Edge::Top);
        assert_eq!(Edge::default_for("windows"), Edge::Bottom);
        let (x, y) = place(&macbook(), None, SIZE, Edge::Top);
        assert_eq!((x, y), (1512.0 - WIDTH - MARGIN, 33.0 + GAP));
        let (x, y) = place(&windows_bottom(), None, SIZE, Edge::Bottom);
        assert_eq!((x, y), (1920.0 - WIDTH - MARGIN, 1032.0 - GAP - 400.0));
    }

    #[test]
    fn a_panel_taller_than_the_screen_starts_at_the_top_of_the_work_area() {
        let tiny = Screen {
            bounds: Rect::new(0.0, 0.0, 300.0, 300.0),
            work: Rect::new(0.0, 25.0, 300.0, 275.0),
        };
        let icon = Rect::new(200.0, 2.0, 22.0, 22.0);
        let (x, y) = place(&tiny, Some(icon), SIZE, Edge::Top);
        assert_eq!(x, MARGIN);
        assert_eq!(y, 25.0 + GAP);
    }

    #[test]
    fn resizing_keeps_the_side_that_touches_the_bar() {
        let icon = Rect::new(1700.0, 1040.0, 32.0, 40.0);
        let s = windows_bottom();
        let small = place(&s, Some(icon), (WIDTH, 300.0), Edge::Bottom);
        let tall = place(&s, Some(icon), (WIDTH, 500.0), Edge::Bottom);
        assert_eq!(
            small.1 + 300.0,
            tall.1 + 500.0,
            "the bottom edge stays above the taskbar"
        );
        let icon = Rect::new(1200.0, 4.0, 26.0, 25.0);
        let small = place(&macbook(), Some(icon), (WIDTH, 300.0), Edge::Top);
        let tall = place(&macbook(), Some(icon), (WIDTH, 500.0), Edge::Top);
        assert_eq!(small.1, tall.1, "the top edge stays under the menu bar");
    }

    #[test]
    fn heights_are_clamped() {
        assert_eq!(clamp_height(100.0), MIN_HEIGHT);
        assert_eq!(clamp_height(400.0), 400.0);
        assert_eq!(clamp_height(2000.0), MAX_HEIGHT);
        assert_eq!(clamp_height(f64::NAN), MIN_HEIGHT);
    }

    #[test]
    fn macos_physical_rects_become_points_per_monitor() {
        let r = Rect::new(2400.0, 8.0, 52.0, 50.0).scaled(2.0);
        assert_eq!(r, Rect::new(1200.0, 4.0, 26.0, 25.0));
        assert_eq!(
            Rect::new(10.0, 10.0, 10.0, 10.0).scaled(0.0),
            Rect::new(10.0, 10.0, 10.0, 10.0)
        );
    }
}
