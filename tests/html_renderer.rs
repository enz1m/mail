//! Process-main-thread entry point for isolated HTML renderer contracts.

#[allow(
    dead_code,
    reason = "The renderer runner includes the reader without unrelated app entry points."
)]
mod mail_html {
    include!("../src/mail_html.rs");
    pub(crate) mod native_checks {
        include!("../src/mail_html/native_checks.rs");
    }
}

fn main() -> std::process::ExitCode {
    let cases: [(&str, fn()); 4] = [
        ("native_dark_html_adapts_important_colors_and_preserves_image_paint", mail_html::native_checks::native_dark_html_adapts_important_colors_and_preserves_image_paint),
        ("native_html_paints_css_and_reserves_blocked_image_dimensions_without_ui_work", mail_html::native_checks::native_html_paints_css_and_reserves_blocked_image_dimensions_without_ui_work),
        ("native_pointer_drag_selects_html_text", mail_html::native_checks::native_pointer_drag_selects_html_text),
        ("native_worker_publishes_selection_highlight_after_pointer_input", mail_html::native_checks::native_worker_publishes_selection_highlight_after_pointer_input)
    ];
    let name = std::env::args().nth(1).unwrap_or_default();
    if name.is_empty() || name.starts_with('-') {
        let executable = std::env::current_exe().expect("renderer test executable");
        for (name, _) in cases {
            let status = std::process::Command::new(&executable)
                .arg(name)
                .status()
                .expect("renderer contract child");
            if !status.success() {
                return std::process::ExitCode::FAILURE;
            }
        }
        return std::process::ExitCode::SUCCESS;
    }
    let Some((_, check)) = cases.iter().find(|(case, _)| *case == name) else {
        eprintln!("Select a named HTML renderer contract or run without arguments for all checks.");
        return std::process::ExitCode::FAILURE;
    };
    check();
    eprintln!("HTML renderer PASS: {name}");
    std::process::ExitCode::SUCCESS
}
