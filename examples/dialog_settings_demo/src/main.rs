//! dialog_settings_demo — Phase 37 C-1 + C-5: native file dialogs + JSON
//! settings persistence.
//!
//! Knot-shaped: load a small `Settings` struct from disk on startup, drive a
//! Theme/Backup-folder/Last-export trio from `Signal`s, and write the struct
//! back via `write_json_atomic`. The two file-dialog buttons exercise
//! `FileDialog::open_folder` and `FileDialog::save_file` against `rfd`.
//!
//! The settings file lives at `<OS-config-dir>/sindon_dialog_settings_demo/
//! settings.json`. Path is shown in the UI so you can verify the round-trip
//! by opening the file after pressing Save.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sindon::app::App;
use sindon::core::Theme;
use sindon::platform::{FileDialog, config_dir, read_json, write_json_atomic};
use sindon::reactive::{Reactive, Signal};
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, TextWidget};

const APP_NAME: &str = "sindon_dialog_settings_demo";

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct Settings {
    /// "dark" or "light". String not enum so future themes don't break old
    /// JSON files.
    #[serde(default = "default_theme")]
    theme: String,
    #[serde(default)]
    backup_folder: Option<PathBuf>,
    #[serde(default)]
    last_export: Option<PathBuf>,
}

fn default_theme() -> String {
    "dark".into()
}

fn main() {
    let dir = config_dir(APP_NAME).expect("failed to acquire OS config dir");
    let settings_path = dir.join("settings.json");

    let loaded: Settings = read_json(&settings_path)
        .unwrap_or_default()
        .unwrap_or_default();

    let theme_choice: Signal<String> = Signal::new(if loaded.theme == "light" {
        "light".into()
    } else {
        "dark".into()
    });
    let backup: Signal<Option<PathBuf>> = Signal::new(loaded.backup_folder.clone());
    let export: Signal<Option<PathBuf>> = Signal::new(loaded.last_export.clone());
    let status: Signal<String> = Signal::new(if settings_path.exists() {
        format!("Loaded settings from {}", settings_path.display())
    } else {
        format!(
            "No settings file yet. Will create at {} on save.",
            settings_path.display()
        )
    });

    let theme_for_app = theme_choice;
    let theme_reactive = Reactive::derive(move || {
        if theme_for_app.get_clone() == "light" {
            Theme::light()
        } else {
            Theme::dark()
        }
    });

    let settings_path_for_save = settings_path.clone();
    let settings_path_for_label = settings_path;

    App::new()
        .title("sindon \u{2014} dialog + settings demo (Phase 37)")
        .size(720, 560)
        .theme(theme_reactive)
        .run(move |_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(24.0)
                    .gap(14.0),
            );

            tree.add_child(
                root,
                TextWidget::new("Phase 37 \u{2014} native dialogs + JSON settings").font_size(22.0),
            );

            tree.add_child(
                root,
                TextWidget::new(format!(
                    "Settings file: {}",
                    settings_path_for_label.display()
                ))
                .font_size(12.0),
            );

            // --- Theme picker --------------------------------------------------
            let theme_row = tree.add_child(root, Container::row().gap(8.0));
            tree.add_child(
                theme_row,
                TextWidget::reactive(move || format!("Theme: {}", theme_choice.get_clone()))
                    .font_size(14.0),
            );
            tree.add_child(
                theme_row,
                Button::new("Dark").radius(6.0).on_click(move |_ctx| {
                    theme_choice.set("dark".into());
                }),
            );
            tree.add_child(
                theme_row,
                Button::new("Light").radius(6.0).on_click(move |_ctx| {
                    theme_choice.set("light".into());
                }),
            );

            // --- Backup folder picker -----------------------------------------
            let backup_row = tree.add_child(root, Container::row().gap(8.0));
            tree.add_child(
                backup_row,
                TextWidget::reactive(move || match backup.get_clone() {
                    Some(p) => format!("Backup folder: {}", p.display()),
                    None => "Backup folder: (none)".into(),
                })
                .font_size(14.0),
            );
            tree.add_child(
                backup_row,
                Button::new("Pick folder\u{2026}")
                    .radius(6.0)
                    .on_click(move |_ctx| {
                        if let Some(path) =
                            FileDialog::new().title("Pick backup folder").open_folder()
                        {
                            backup.set(Some(path));
                        }
                    }),
            );

            // --- Export-path picker -------------------------------------------
            let export_row = tree.add_child(root, Container::row().gap(8.0));
            tree.add_child(
                export_row,
                TextWidget::reactive(move || match export.get_clone() {
                    Some(p) => format!("Last export: {}", p.display()),
                    None => "Last export: (none)".into(),
                })
                .font_size(14.0),
            );
            tree.add_child(
                export_row,
                Button::new("Pick save path\u{2026}")
                    .radius(6.0)
                    .on_click(move |_ctx| {
                        if let Some(path) = FileDialog::new()
                            .title("Export note as\u{2026}")
                            .filter("Markdown", &["md", "markdown"])
                            .filter("Text", &["txt"])
                            .file_name("untitled.md")
                            .save_file()
                        {
                            export.set(Some(path));
                        }
                    }),
            );

            // --- Save / Reset --------------------------------------------------
            let action_row = tree.add_child(root, Container::row().gap(8.0));
            let save_path = settings_path_for_save.clone();
            tree.add_child(
                action_row,
                Button::new("Save settings")
                    .radius(6.0)
                    .on_click(move |_ctx| {
                        let snapshot = Settings {
                            theme: theme_choice.get_clone(),
                            backup_folder: backup.get_clone(),
                            last_export: export.get_clone(),
                        };
                        match write_json_atomic(&save_path, &snapshot) {
                            Ok(()) => {
                                status.set(format!("Saved to {}", save_path.display()));
                            }
                            Err(e) => {
                                status.set(format!("Save failed: {e}"));
                            }
                        }
                    }),
            );
            tree.add_child(
                action_row,
                Button::new("Reset fields")
                    .radius(6.0)
                    .on_click(move |_ctx| {
                        theme_choice.set("dark".into());
                        backup.set(None);
                        export.set(None);
                        status.set(
                            "Fields reset (settings file untouched until you press Save).".into(),
                        );
                    }),
            );

            tree.add_child(
                root,
                TextWidget::reactive(move || status.get_clone()).font_size(12.0),
            );

            tree.add_child(
                root,
                TextWidget::new(
                    "Save writes via fs::rename so readers never see a partial file. \
                     Theme is read back on next launch \u{2014} run the demo twice to \
                     verify the round-trip.",
                )
                .font_size(11.0),
            );

            tree
        });
}
