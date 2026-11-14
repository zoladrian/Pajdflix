use crate::MainWindow;
use crate::config::AppConfig;

fn show_release_notes(ui: &MainWindow) {
    // Display the shipped Pajdflix notes even offline. Upstream releases have
    // their own numbering and are not changes in this product.
    let text = format!(
        "# Pajdflix {}\n\n{}",
        env!("CARGO_PKG_VERSION"),
        include_str!("../../assets/release-notes.md")
    );
    let styled = slint::StyledText::from_markdown(&text)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(&text));
    ui.set_onboarding_changelog_text(styled);
}

pub fn setup(ui: &MainWindow, config: &AppConfig) {
    show_release_notes(ui);

    let force_onboarding = std::env::args().any(|arg| {
        arg == "--force-onboarding" || arg == "--onboarding" || arg == "--test-onboarding"
    });

    // Show onboarding modal on first launch or when forced via CLI argument
    if force_onboarding || !config.onboarding_completed {
        tracing::info!(
            force = force_onboarding,
            "Triggering onboarding wizard popup"
        );
        ui.set_onboarding_dialog_open(true);
    }

    // Bind onboarding completed callback
    ui.on_onboarding_completed({
        move || {
            let mut cfg = crate::config::load_config();
            cfg.onboarding_completed = true;
            crate::config::save_config(&cfg);
            tracing::info!("Onboarding completed and saved to configuration.");
        }
    });
}
