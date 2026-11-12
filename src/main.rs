// mongui: a small, fast MongoDB browser.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod conn;
mod jks;
mod shell;
mod worker;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("mongui")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            .with_icon(
                // include_bytes! embeds the PNG into the binary at compile time.
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/mongui-icon.png"))
                    .expect("assets/mongui-icon.png is a valid PNG"),
            ),
        ..Default::default()
    };
    eframe::run_native("mongui", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
