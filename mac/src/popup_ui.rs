//! The right-click menu and the hover card, drawn in the same style as the usage window.

use crate::app::{self, Command};
use crate::canvas::{self, mix, Align, Font, WEIGHT_BOLD, WEIGHT_REGULAR, WEIGHT_SEMIBOLD};
use crate::clock::today;
use crate::surface::{self, main_thread, GaugeView, KEY_DOWN, KEY_ESCAPE, KEY_RETURN, KEY_UP};
use crate::theme::{appearance_for, current_theme, figure_colors, Theme};
use crate::usage_ui::{self, OpenedBy};
use crate::settings_ui;
use netspeedgauge_shared::format::format_bytes;
use netspeedgauge_shared::usage_store::{Period, UsageStore};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{sel, MainThreadOnly};
use objc2_app_kit::{NSAppearanceCustomization, NSApplication, NSMenu, NSMenuItem, NSWindow};
use objc2_foundation::{NSPoint, NSString};
use std::sync::atomic::{AtomicI64, Ordering};
use std::cell::RefCell;

const CARD_WIDTH: i32 = 216;
const CARD_HEIGHT: i32 = 124;
const CORNER_RADIUS: f32 = 9.0;
// The menu's numbers at full size. It draws them at MENU_SCALE when its text is the default size, and in
// proportion to the text size otherwise, the same as Juice Bar's menu.
const MENU_WIDTH: f64 = 164.0;
const SUMMARY_HEIGHT: f64 = 34.0;
const ROW_HEIGHT: f64 = 32.0;
const DIVIDER_HEIGHT: f64 = 9.0;
const PADDING: f64 = 5.0;
const HOVER_RADIUS: f64 = 5.0;
const ICON_POINTS: f64 = 10.0;
const ICON_CENTER_X: f64 = 22.0;
const LABEL_X: f64 = 40.0;
const SIDE_INSET: f64 = 14.0;
// The least space between a row's label and its shortcut, or between "Today" and its total.
const NAME_GAP: f64 = 24.0;
const MENU_SCALE: f64 = 0.8;
const MENU_FONT_POINTS_DEFAULT: i64 = 9;
// The header is one point under the rows' text.
const HEADER_POINTS_BELOW: f64 = 1.0;
// The Mac's own panels, measured on Weather, start one point above the bottom of the menu bar.
const MENU_OVERLAP_WITH_BAR: f64 = 1.0;
// The selected background ends 1 point past the item, and 2 more makes the rounded corner look lined up with it.
const MENU_PAST_ITEM: f64 = 3.0;
const ARROW_UP: &str = "\u{2191}";
const ARROW_DOWN: &str = "\u{2193}";

/// One thing the menu can do: its icon, label, the work it asks for, and its key with Command (none for Usage).
const MENU_ITEMS: [(&str, &str, Command, &str); 3] = [
    ("chart.bar", "Usage", Command::OpenUsage(OpenedBy::Menu), ""),
    ("gearshape", "Settings", Command::OpenSettings, ","),
    ("power", "Quit", Command::Quit, "q"),
];
const QUIT_ROW: usize = 2;

// The menu's text size, set by the app from the settings whenever they change.
static MENU_FONT_POINTS: AtomicI64 = AtomicI64::new(MENU_FONT_POINTS_DEFAULT);

/// Sets the right-click menu's text size, in points.
pub fn set_menu_font_points(points: i64) {
    MENU_FONT_POINTS.store(points, Ordering::Relaxed);
}

/// Returns the shortcut as the menu shows it, such as "⌘,".
fn shortcut_label(key: &str) -> String {
    if key.is_empty() {
        String::new()
    } else {
        format!("\u{2318}{}", key.to_uppercase())
    }
}

/// The menu's fonts and sizes for one text size. Every length is the full size one scaled by the same factor.
struct MenuMetrics {
    body: Font,
    strong: Font,
    small: Font,
    width: i32,
    summary_height: i32,
    row_height: i32,
    divider_height: i32,
    padding: i32,
    hover_radius: f32,
    icon_points: f64,
    icon_center_x: i32,
    label_x: i32,
    label_top: i32,
    header_top: i32,
    total_top: i32,
    side_inset: i32,
}

impl MenuMetrics {
    fn new(font_points: i64, totals: &Totals) -> Self {
        let points = font_points.clamp(7, 12) as f64;
        let scale = MENU_SCALE * points / MENU_FONT_POINTS_DEFAULT as f64;
        let length = |value: f64| (value * scale).round() as i32;
        let body = Font::system(points, WEIGHT_REGULAR);
        let strong = Font::system(points, WEIGHT_SEMIBOLD);
        let small = Font::system(points - HEADER_POINTS_BELOW, WEIGHT_REGULAR);
        let (summary_height, row_height) = (length(SUMMARY_HEIGHT), length(ROW_HEIGHT));
        let (label_x, side_inset, gap) = (length(LABEL_X), length(SIDE_INSET), length(NAME_GAP));
        // Wide enough for the longest label and its shortcut, and for "Today" and its total, each with the gap.
        let rows = MENU_ITEMS
            .iter()
            .map(|(_, label, _, key)| label_x + canvas::text_width(label, &strong) + gap + canvas::text_width(&shortcut_label(key), &body) + side_inset)
            .max()
            .unwrap_or(0);
        let header = side_inset * 2 + canvas::text_width("Today", &small) + gap + canvas::text_width(&format_bytes(totals.today_up + totals.today_down), &strong);
        Self {
            width: length(MENU_WIDTH).max(rows).max(header),
            label_top: (row_height - body.box_height) / 2,
            header_top: (summary_height - small.box_height) / 2,
            total_top: (summary_height - strong.box_height) / 2,
            body,
            strong,
            small,
            summary_height,
            row_height,
            divider_height: length(DIVIDER_HEIGHT),
            padding: length(PADDING),
            hover_radius: (HOVER_RADIUS * scale) as f32,
            icon_points: ICON_POINTS * scale,
            icon_center_x: length(ICON_CENTER_X),
            label_x,
            side_inset,
        }
    }

    /// Returns the y position of a menu row. Quit sits under a divider.
    fn row_top(&self, row: usize) -> i32 {
        let divider = if row >= QUIT_ROW { self.divider_height } else { 0 };
        self.summary_height + self.padding + row as i32 * self.row_height + divider
    }

    fn height(&self) -> i32 {
        self.summary_height + self.padding * 2 + self.row_height * MENU_ITEMS.len() as i32 + self.divider_height
    }
}

struct PopupFonts {
    small: Font,
    value: Font,
    arrow: Font,
}

impl PopupFonts {
    fn new() -> Self {
        Self {
            small: Font::system(9.0, WEIGHT_REGULAR),
            value: Font::system(14.0, WEIGHT_SEMIBOLD),
            arrow: Font::system(10.0, WEIGHT_BOLD),
        }
    }
}

/// Today's and this month's totals, read once when the popup opens.
#[derive(Clone, Copy)]
struct Totals {
    today_up: u64,
    today_down: u64,
    month_up: u64,
    month_down: u64,
}

fn read_totals(store: &UsageStore) -> Totals {
    let day = today();
    let today_bucket = store.buckets(Period::Day, None, day)[Period::Day.bucket_count() - 1];
    let month_bucket = store.buckets(Period::Month, None, day)[Period::Month.bucket_count() - 1];
    Totals { today_up: today_bucket.up, today_down: today_bucket.down, month_up: month_bucket.up, month_down: month_bucket.down }
}

/// What holds a popup on screen. A click uses the Mac's own menu, which gives the same selected
/// background, opening, closing and press-and-hold behaviour as every other menu bar item.
enum PopupHost {
    Window(Retained<NSWindow>),
    SystemMenu(Retained<NSMenu>),
}

/// The open popup. `is_menu` tells the menu from the hover card.
struct Popup {
    host: PopupHost,
    view: Retained<GaugeView>,
    fonts: PopupFonts,
    theme: Theme,
    width: i32,
    height: i32,
    totals: Totals,
    is_menu: bool,
    // A card opened by a click stays until the next click, here or anywhere else.
    is_pinned: bool,
    hover_row: Option<usize>,
    menu: MenuMetrics,
}

thread_local! {
    static OPEN_POPUP: RefCell<Option<Popup>> = const { RefCell::new(None) };
    static SYSTEM_MENU: std::cell::OnceCell<Retained<NSMenu>> = const { std::cell::OnceCell::new() };
    // What a menu row asked for, waiting until the menu has fully closed so its window gets the focus.
    static PENDING_COMMAND: RefCell<Option<Command>> = const { RefCell::new(None) };
}

fn with_popup(action: impl FnOnce(&mut Popup)) {
    OPEN_POPUP.with(|cell| {
        if let Ok(mut state) = cell.try_borrow_mut() {
            if let Some(popup) = state.as_mut() {
                action(popup);
            }
        }
    });
}

impl Popup {
    fn is_held_by(&self, window: &NSWindow) -> bool {
        matches!(&self.host, PopupHost::Window(held) if std::ptr::eq(&**held, window))
    }

    fn row_at(&self, y: i32) -> Option<usize> {
        if !self.is_menu {
            return None;
        }
        (0..MENU_ITEMS.len()).find(|&row| y >= self.menu.row_top(row) && y < self.menu.row_top(row) + self.menu.row_height)
    }

    /// Draws the labelled download and upload totals used by both popups.
    fn draw_totals(&self, y: i32, label: &str, up: u64, down: u64, arrow_right: i32) {
        let theme = self.theme;
        let left = 16;
        let right = self.width - 16;
        canvas::text(left, y, label, &self.fonts.small, theme.muted, Align::Left);
        canvas::text(left, y + 16, &format_bytes(up + down), &self.fonts.value, theme.text, Align::Left);
        let (upload, download) = figure_colors(&theme, theme.surface);
        for (arrow, color, bytes, row_y) in [(ARROW_DOWN, download, down, y + 4), (ARROW_UP, upload, up, y + 24)] {
            canvas::text(right, row_y, &format_bytes(bytes), &self.fonts.small, theme.text, Align::Right);
            canvas::text(arrow_right, row_y, arrow, &self.fonts.arrow, color, Align::Right);
        }
    }

    fn draw(&self) {
        let theme = self.theme;
        let totals = self.totals;
        // The Mac's own menu paints its background, so only a window paints one here.
        if matches!(self.host, PopupHost::Window(_)) {
            canvas::round_rect(0, 0, self.width, self.height, CORNER_RADIUS, theme.border);
            canvas::round_rect(1, 1, self.width - 2, self.height - 2, CORNER_RADIUS - 1.0, theme.surface);
        }
        if !self.is_menu {
            // Every arrow shares one column, set by the widest number, so they line up at any speed.
            let widest = [totals.today_up, totals.today_down, totals.month_up, totals.month_down]
                .iter()
                .map(|bytes| canvas::text_width(&format_bytes(*bytes), &self.fonts.small))
                .max()
                .unwrap_or(0);
            let arrow_right = self.width - 16 - widest - 10;
            self.draw_totals(12, "Today", totals.today_up, totals.today_down, arrow_right);
            canvas::round_rect(16, 62, self.width - 32, 1, 0.0, theme.border);
            self.draw_totals(72, "This month", totals.month_up, totals.month_down, arrow_right);
            return;
        }
        let menu = &self.menu;
        let today_total = format_bytes(totals.today_up + totals.today_down);
        // The total, the shortcuts and nothing else share one right edge, as in Juice Bar's menu.
        let right = self.width - menu.side_inset;
        canvas::text(menu.side_inset, menu.header_top, "Today", &menu.small, theme.muted, Align::Left);
        canvas::text_ink_right(right, menu.total_top, &today_total, &menu.strong, theme.text);
        let divider_width = self.width - menu.padding * 2;
        canvas::round_rect(menu.padding, menu.summary_height, divider_width, 1, 0.0, theme.border);
        for (row, (icon, label, _, key)) in MENU_ITEMS.iter().enumerate() {
            let top = menu.row_top(row);
            let is_hover = self.hover_row == Some(row);
            if is_hover {
                canvas::round_rect(menu.padding, top, divider_width, menu.row_height, menu.hover_radius, mix(theme.surface, theme.text, 0.09));
            }
            canvas::symbol(menu.icon_center_x, top + menu.row_height / 2, icon, menu.icon_points, if is_hover { theme.text } else { theme.muted });
            let font = if is_hover { &menu.strong } else { &menu.body };
            canvas::text(menu.label_x, top + menu.label_top, label, font, theme.text, Align::Left);
            if !key.is_empty() {
                canvas::text_ink_right(right, top + menu.label_top, &shortcut_label(key), &menu.body, theme.muted);
            }
        }
        canvas::round_rect(menu.padding, menu.row_top(QUIT_ROW) - (menu.divider_height / 2 + 1), divider_width, 1, 0.0, theme.border);
    }

    fn invalidate(&self) {
        self.view.setNeedsDisplay(true);
    }

    fn set_hover(&mut self, row: Option<usize>) {
        if row != self.hover_row {
            self.hover_row = row;
            self.invalidate();
        }
    }

    /// Moves the keyboard highlight, wrapping around the ends.
    fn move_hover(&mut self, step: usize) {
        let count = MENU_ITEMS.len();
        let next = self.hover_row.map_or(if step == 1 { 0 } else { count - 1 }, |row| (row + step) % count);
        self.set_hover(Some(next));
    }
}

/// Asks the app to do what a menu row says, then closes the menu.
fn choose(row: usize) {
    let mut is_system_menu = false;
    with_popup(|popup| is_system_menu = matches!(popup.host, PopupHost::SystemMenu(_)));
    if is_system_menu {
        PENDING_COMMAND.with(|cell| *cell.borrow_mut() = Some(MENU_ITEMS[row].2));
    } else {
        app::post(MENU_ITEMS[row].2);
    }
    app::post(Command::ClosePopup);
}

/// Paints the open popup.
pub fn draw() {
    with_popup(|popup| popup.draw());
}

/// Reacts to the mouse moving and returns true when it is over something clickable.
pub fn on_mouse_move(_x: i32, y: i32) -> bool {
    let mut is_clickable = false;
    with_popup(|popup| {
        let row = popup.row_at(y);
        popup.set_hover(row);
        is_clickable = row.is_some();
    });
    is_clickable
}

pub fn on_mouse_exit() {
    with_popup(|popup| popup.set_hover(None));
}

pub fn on_click(_x: i32, y: i32) {
    let mut chosen = None;
    with_popup(|popup| chosen = popup.row_at(y));
    if let Some(row) = chosen {
        choose(row);
    }
}

pub fn on_key(code: u16) {
    let mut chosen = None;
    match code {
        KEY_ESCAPE => app::post(Command::ClosePopup),
        KEY_DOWN => with_popup(|popup| popup.move_hover(1)),
        KEY_UP => with_popup(|popup| popup.move_hover(MENU_ITEMS.len() - 1)),
        KEY_RETURN => with_popup(|popup| chosen = popup.hover_row),
        _ => {}
    }
    if let Some(row) = chosen {
        choose(row);
    }
}

/// A menu or a pinned card closes as soon as the user clicks anywhere else.
pub fn on_window_resigned(window: &NSWindow) {
    with_popup(|popup| {
        if popup.is_held_by(window) {
            if popup.is_pinned {
                app::note_auto_close();
            }
            app::post(Command::ClosePopup);
        }
    });
}

/// Frees the popup once its window has closed, and hands the keyboard back if nothing else is open.
pub fn on_window_closed(window: &NSWindow) {
    let mut closed = None;
    OPEN_POPUP.with(|cell| {
        if let Ok(mut state) = cell.try_borrow_mut() {
            if state.as_ref().is_some_and(|popup| popup.is_held_by(window)) {
                closed = state.take();
            }
        }
    });
    let Some(popup) = closed else {
        return;
    };
    let took_focus = popup.is_menu || popup.is_pinned;
    if let PopupHost::Window(window) = &popup.host {
        surface::hold_closed_window(window.clone());
    }
    if took_focus && usage_ui::opened_by().is_none() && !settings_ui::is_open() && !surface::is_headless() {
        NSApplication::sharedApplication(main_thread()).deactivate();
    }
}

fn popup_size(is_menu: bool, menu: &MenuMetrics) -> (i32, i32) {
    if is_menu {
        (menu.width, menu.height())
    } else {
        (CARD_WIDTH, CARD_HEIGHT)
    }
}

fn menu_metrics(totals: &Totals) -> MenuMetrics {
    MenuMetrics::new(MENU_FONT_POINTS.load(Ordering::Relaxed), totals)
}

/// Opens a popup just under the menu bar item.
fn open(store: &UsageStore, is_menu: bool, is_pinned: bool) {
    close();
    let totals = read_totals(store);
    let menu = menu_metrics(&totals);
    let (width, height) = popup_size(is_menu, &menu);
    let takes_focus = is_menu || is_pinned;
    let (window, view) = surface::create_popup(width, height);
    app::place_below_item(&window);
    let popup = new_popup(PopupHost::Window(window.clone()), view.clone(), current_theme(), (width, height), totals, (is_menu, is_pinned), menu);
    OPEN_POPUP.with(|cell| *cell.borrow_mut() = Some(popup));
    if takes_focus {
        surface::show_with_focus(&window, &view);
    } else {
        surface::show_without_focus(&window);
    }
}

fn new_popup(host: PopupHost, view: Retained<GaugeView>, theme: Theme, size: (i32, i32), totals: Totals, (is_menu, is_pinned): (bool, bool), menu: MenuMetrics) -> Popup {
    let (width, height) = size;
    Popup { host, view, fonts: PopupFonts::new(), theme, width, height, totals, is_menu, is_pinned, hover_row: None, menu }
}

/// Places the Mac's own menu just under the menu bar item, with its right edge at the end of the selected background.
pub fn on_will_draw_in(window: &NSWindow) {
    let mut is_system_menu = false;
    with_popup(|popup| is_system_menu = matches!(popup.host, PopupHost::SystemMenu(_)));
    if is_system_menu {
        let size = window.frame().size;
        let area = app::screen_area();
        let anchor = app::anchor_frame();
        let top = area.origin.y + area.size.height + MENU_OVERLAP_WITH_BAR;
        let right = anchor.origin.x + anchor.size.width + MENU_PAST_ITEM;
        let origin = NSPoint::new((right - size.width).max(area.origin.x), top - size.height);
        if window.frame().origin != origin {
            window.setFrameOrigin(origin);
        }
    }
}

/// Returns the Mac's own menu that stays on the menu bar item. Its content is built each time it opens.
pub fn system_menu() -> Retained<NSMenu> {
    SYSTEM_MENU.with(|cell| {
        cell.get_or_init(|| {
            let menu = NSMenu::new(main_thread());
            menu.addItem(&NSMenuItem::new(main_thread()));
            menu.setDelegate(Some(ProtocolObject::from_ref(&*surface::delegate())));
            menu
        })
        .clone()
    })
}

/// Gives the Mac's own menu the chosen light or dark look. Setting it as the menu opens is too late for that opening.
pub fn sync_system_menu_look() {
    system_menu().setAppearance(appearance_for(&current_theme()).as_deref());
}

/// Builds the card or the right-click menu inside the Mac's own menu, just before it opens.
pub fn fill_system_menu(store: &UsageStore, is_menu: bool) {
    close();
    let menu = system_menu();
    let totals = read_totals(store);
    let metrics = menu_metrics(&totals);
    let (width, height) = popup_size(is_menu, &metrics);
    let view = GaugeView::create(surface::Surface::Popup, width, height);
    let item = NSMenuItem::new(main_thread());
    item.setView(Some(&view));
    menu.removeAllItems();
    menu.addItem(&item);
    if is_menu {
        add_shortcut_items(&menu);
    }
    let popup = new_popup(PopupHost::SystemMenu(menu), view, current_theme(), (width, height), totals, (is_menu, !is_menu), metrics);
    OPEN_POPUP.with(|cell| *cell.borrow_mut() = Some(popup));
}

/// Adds a hidden item for each row with a shortcut, so ⌘, and ⌘Q work while the menu is open. The drawn
/// view shows the rows; these only answer the keys.
fn add_shortcut_items(menu: &NSMenu) {
    for (row, (_, label, _, key)) in MENU_ITEMS.iter().enumerate().filter(|(_, item)| !item.3.is_empty()) {
        // SAFETY: the delegate answers menuShortcut: and lives for the whole app.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(main_thread()), &NSString::from_str(label), Some(sel!(menuShortcut:)), &NSString::from_str(key))
        };
        let target = surface::delegate();
        let target: &AnyObject = target.as_ref();
        // SAFETY: as above.
        unsafe { item.setTarget(Some(target)) };
        item.setTag(row as isize);
        item.setHidden(true);
        item.setAllowsKeyEquivalentWhenHidden(true);
        menu.addItem(&item);
    }
}

/// Runs a menu row from its shortcut, the same as clicking it.
pub fn on_shortcut(row: isize) {
    if let Ok(row) = usize::try_from(row) {
        if row < MENU_ITEMS.len() {
            choose(row);
        }
    }
}

/// Runs a menu row from Command and a key pressed in a menu shown in a window, as the Mac's own menu does.
pub fn on_command_key(key: &str) {
    let mut is_menu = false;
    with_popup(|popup| is_menu = popup.is_menu);
    if let Some(row) = MENU_ITEMS.iter().position(|item| is_menu && !item.3.is_empty() && item.3.eq_ignore_ascii_case(key)) {
        choose(row);
    }
}

/// Forgets the popup once the Mac's own menu has closed, and runs what a row asked for just after.
pub fn on_system_menu_closed() {
    OPEN_POPUP.with(|cell| cell.borrow_mut().take());
    if PENDING_COMMAND.with(|cell| cell.borrow().is_some()) {
        surface::run_pending_command_soon();
    }
}

/// Runs what a menu row asked for, now that the menu has fully closed.
pub fn run_pending_command() {
    if let Some(command) = PENDING_COMMAND.with(|cell| cell.borrow_mut().take()) {
        app::post(command);
    }
}

/// Opens the right-click menu.
pub fn show_menu(store: &UsageStore) {
    open(store, true, false);
}

/// Opens the card for a click. Unlike the hover card, it stays when the mouse moves away.
pub fn show_pinned_card(store: &UsageStore) {
    open(store, false, true);
}

/// Opens the hover card with today's and this month's totals, unless a popup is showing.
pub fn show_hover_card(store: &UsageStore) {
    if !is_open() {
        open(store, false, false);
    }
}

/// Returns true while a card opened by a click is showing, so the next click can close it.
pub fn is_pinned_card_open() -> bool {
    let mut is_open = false;
    with_popup(|popup| is_open = popup.is_pinned);
    is_open
}

/// Returns true while any popup is showing.
pub fn is_open() -> bool {
    let mut is_open = false;
    with_popup(|_| is_open = true);
    is_open
}

/// Returns true while the menu, rather than a card, is showing.
pub fn is_menu_open() -> bool {
    let mut is_open = false;
    with_popup(|popup| is_open = popup.is_menu);
    is_open
}

/// Closes the hover card when the mouse leaves, leaving a menu or a pinned card open.
pub fn hide_hover_card() {
    with_popup(|popup| {
        if !popup.is_menu && !popup.is_pinned {
            app::post(Command::ClosePopup);
        }
    });
}

/// Closes whichever popup is open.
pub fn close() {
    let mut window = None;
    let mut menu = None;
    with_popup(|popup| match &popup.host {
        PopupHost::Window(held) => window = Some(held.clone()),
        PopupHost::SystemMenu(held) => menu = Some(held.clone()),
    });
    if let Some(window) = window {
        window.close();
    }
    if let Some(menu) = menu {
        menu.cancelTracking();
    }
}

/// Paints the open popup to a PNG file, for checks.
pub fn capture(path: &str) -> bool {
    // The state must be free while painting, since painting reads it.
    let mut view = None;
    with_popup(|popup| view = Some(popup.view.clone()));
    view.is_some_and(|view| surface::capture_view(&view, path))
}

/// Shows the open popup in a chosen theme, for checks.
pub fn preview_theme(is_dark: bool) {
    with_popup(|popup| popup.theme = if is_dark { crate::theme::dark_theme() } else { crate::theme::light_theme() });
}
