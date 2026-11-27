//! Paints every screen to PNG files without showing anything, using made-up usage in a throwaway home folder.
//! Run with: cargo run -p netspeedgauge-mac --example capture_views -- <output folder>

#[cfg(target_os = "macos")]
fn main() {
    use netspeedgauge_mac::clock::today;
    use netspeedgauge_mac::readout::Readout;
    use netspeedgauge_mac::surface;
    use netspeedgauge_mac::usage_ui::OpenedBy;
    use netspeedgauge_mac::{app, popup_ui, settings_ui, usage_ui};
    use netspeedgauge_shared::settings::Field;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let output = std::env::args().nth(1).expect("give an output folder");
    std::fs::create_dir_all(&output).expect("output folder");
    let home = std::env::temp_dir().join(format!("netspeedgauge-capture-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("temp folder");
    std::env::set_var("HOME", &home);
    surface::set_headless();
    let mtm = MainThreadMarker::new().expect("runs on the main thread");
    NSApplication::sharedApplication(mtm).setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    app::start_without_menu_bar_item();
    let (usage, settings) = app::state_for_checks().expect("started");
    {
        let mut store = usage.borrow_mut();
        store.adapters.clear();
        store.records.clear();
        store.add_adapter(1, "Wi-Fi".to_string());
        store.add_adapter(2, "USB 10/100/1000 LAN".to_string());
        let day = today();
        for back in 0..420 {
            let wave = u64::from(((back * 37) % 23 + 2) as u32);
            store.record(1, day - back, wave * 90_000_000, wave * 1_400_000_000);
            if back % 3 == 0 {
                store.record(2, day - back, wave * 30_000_000, wave * 300_000_000);
            }
        }
    }
    let path = |name: &str| format!("{output}/{name}.png");

    for (theme, is_dark) in [("dark", true), ("light", false)] {
        settings.borrow_mut().set(Field::WindowTheme, if is_dark { 2 } else { 1 });
        netspeedgauge_mac::theme::set_theme_choice(if is_dark { 2 } else { 1 });

        usage_ui::open(usage.clone(), false, OpenedBy::Menu);
        for (position, name) in ["day", "week", "month", "year"].iter().enumerate() {
            usage_ui::click_for_checks("period", position);
            assert!(usage_ui::capture(&path(&format!("usage_{name}_{theme}"))));
        }
        usage_ui::click_for_checks("period", 0);
        usage_ui::click_for_checks("adapter", 0);
        assert!(usage_ui::capture(&path(&format!("usage_adapter_menu_{theme}"))));
        usage_ui::close();

        popup_ui::show_hover_card(&usage.borrow());
        assert!(popup_ui::capture(&path(&format!("card_{theme}"))));
        popup_ui::close();
        popup_ui::show_menu(&usage.borrow());
        assert!(popup_ui::capture(&path(&format!("menu_{theme}"))));
        popup_ui::close();
        // The menu at its smallest, default and largest text sizes, plain and with Settings selected.
        for points in [7, 9, 12] {
            popup_ui::set_menu_font_points(points);
            popup_ui::show_menu(&usage.borrow());
            let plain = path(&format!("menu_{points}pt_{theme}"));
            assert!(popup_ui::capture(&plain));
            popup_ui::on_key(surface::KEY_DOWN);
            popup_ui::on_key(surface::KEY_DOWN);
            assert!(popup_ui::capture(&path(&format!("menu_{points}pt_{theme}_hover"))));
            popup_ui::close();
            app::flush_commands();
            let edges = right_column_edges(&plain);
            println!("menu {points} pt {theme}: right column ink ends at pixels {edges:?}");
            assert!(edges.len() >= 3 && edges.windows(2).all(|pair| pair[0] == pair[1]), "the total and shortcuts must share one right edge");
        }
        // The card follows the same text size as the menu.
        for points in [7, 9, 12] {
            popup_ui::set_menu_font_points(points);
            popup_ui::show_hover_card(&usage.borrow());
            assert!(popup_ui::capture(&path(&format!("card_{points}pt_{theme}"))));
            popup_ui::close();
            app::flush_commands();
        }
        popup_ui::set_menu_font_points(9);

        settings_ui::open(settings.clone(), false);
        for (page, name) in ["general", "appearance", "units", "network", "advanced", "details"].iter().enumerate() {
            settings_ui::show_page_for_checks(page);
            assert!(settings_ui::capture(&path(&format!("settings_{name}_{theme}"))));
        }
        // Both resets ask on a card, then show a green done line.
        for name in ["reset_page", "reset_all"] {
            settings_ui::show_page_for_checks(4);
            settings_ui::capture(&path("_measure"));
            assert!(settings_ui::click_reset_for_checks(name));
            assert!(settings_ui::capture(&path(&format!("settings_{name}_confirm_{theme}"))));
            assert!(settings_ui::click_reset_for_checks("confirm_reset"));
            app::flush_commands();
            assert!(settings_ui::capture(&path(&format!("settings_{name}_done_{theme}"))));
            settings_ui::hide_reset_done();
        }
        settings_ui::show_page_for_checks(1);
        settings_ui::capture(&path("_measure"));
        settings_ui::scroll_for_checks(10_000);
        settings_ui::capture(&path("_measure"));
        assert!(settings_ui::capture(&path(&format!("settings_appearance_end_{theme}"))));
        settings_ui::scroll_for_checks(120);
        settings_ui::capture(&path("_measure"));
        assert!(settings_ui::capture_with_title_bar(&path(&format!("settings_scrolled_title_bar_{theme}"))));
        settings_ui::close();
        app::flush_commands();
        surface::release_closed_windows();

        // The menu bar item, on a window of its own so it can be painted without a real menu bar.
        let current = settings.borrow().clone();
        let readout = Readout::new(&current);
        println!("menu bar item width at default settings: {} points", readout.width);
    }
    let _ = std::fs::remove_file(path("_measure"));
    let _ = std::fs::remove_dir_all(&home);
    println!("captured to {output}");
}

/// Returns where the ink ends in each band of lines with ink in a PNG's right third. Bands one or two
/// pixels tall are divider lines, which run the full width, so they are skipped.
#[cfg(target_os = "macos")]
fn right_column_edges(path: &str) -> Vec<isize> {
    use objc2_app_kit::NSBitmapImageRep;
    use objc2_foundation::{NSData, NSString};
    let data = NSData::dataWithContentsOfFile(&NSString::from_str(path)).expect("png");
    let image = NSBitmapImageRep::imageRepWithData(&data).expect("bitmap");
    let (wide, high) = (image.pixelsWide(), image.pixelsHigh());
    // The middle of the menu, clear of its rounded edge, is the surface colour.
    let background = image.colorAtX_y(wide / 2, high - 4).expect("pixel");
    let is_ink = |x: isize, y: isize| {
        let pixel = image.colorAtX_y(x, y).expect("pixel");
        (pixel.redComponent() - background.redComponent()).abs() + (pixel.greenComponent() - background.greenComponent()).abs() + (pixel.blueComponent() - background.blueComponent()).abs()
            > 0.25
    };
    // Stay inside the 1 point border the window menu draws round itself.
    let (column, last) = (wide * 2 / 3, wide - 3);
    let mut edges = Vec::new();
    let mut band: Option<(isize, isize)> = None;
    for y in 3..=high - 3 {
        let right = if y < high - 3 { (column..last).rev().find(|&x| is_ink(x, y)) } else { None };
        match (right, band) {
            (Some(x), Some((edge, tall))) => band = Some((edge.max(x + 1), tall + 1)),
            (Some(x), None) => band = Some((x + 1, 1)),
            (None, Some((edge, tall))) => {
                if tall > 2 {
                    edges.push(edge);
                }
                band = None;
            }
            (None, None) => {}
        }
    }
    edges
}

#[cfg(not(target_os = "macos"))]
fn main() {}
