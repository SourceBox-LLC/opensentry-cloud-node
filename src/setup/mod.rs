// OpenSentry CloudNode - Camera streaming node for OpenSentry Cloud
// Copyright (C) 2026  SourceBox LLC
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.
//! Setup wizard for OpenSentry CloudNode
//!
//! Beautiful animated terminal-based setup experience

pub mod animations;
pub mod platform;
pub mod recovery;
pub mod tui;
pub mod ui;
pub mod validator;

use anyhow::Result;
use std::path::PathBuf;

pub use animations::{
    animate_rainbow_text, clear_screen, draw_box, draw_expanding_border, fade_in, fade_in_lines,
    print_centered, pulse_text, rainbow_text, rainbow_text_offset, show_confetti,
    show_mini_celebration, Spinner,
};
pub use platform::PlatformInfo;
pub use recovery::{show_registration_error, show_success, show_warning, RegistrationError};
pub use tui::run_tui_setup;
pub use validator::ValidationResult;

/// Setup configuration
#[derive(Debug, Clone)]
pub struct SetupConfig {
    pub node_id: String,
    pub api_key: String,
    pub api_url: String,
    pub deployment_method: DeploymentMethod,
    pub output_dir: PathBuf,
    pub auto_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeploymentMethod {
    WindowsNative,
    WSL2,
    LinuxNative,
    Docker,
}

/// Run interactive setup wizard
/// Returns: Ok(true) if should auto-start, Ok(false) if should wait for user
pub fn run_setup() -> Result<bool> {
    // Always try TUI setup - works when double-clicked on Windows
    // The TUI uses inquire which handles both terminal and pipe scenarios
    match tui::run_tui_setup() {
        Ok(auto_start) => Ok(auto_start),
        Err(e) => {
            // If TUI fails, show simple prompt-based setup
            eprintln!("\n  Interactive setup failed: {}", e);
            eprintln!("  This can happen when running without a proper terminal.");
            eprintln!("\n  To run setup, open a terminal and run:");
            eprintln!("    opensentry-cloudnode.exe setup");
            eprintln!("\n  Config is stored in data/node.db (run setup to configure).");
            
            // Pause on Windows so user sees the error before window closes
            #[cfg(target_os = "windows")]
            {
                eprintln!("\n  Press Enter to exit...");
                let _ = std::io::stdin().read_line(&mut String::new());
            }
            
            std::process::exit(1);
        }
    }
}
