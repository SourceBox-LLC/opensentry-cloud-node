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
//! Error recovery and user-friendly error messages
//!
//! Provides clear error messages with recovery suggestions for common failures

use anyhow::Result;
use colored::Colorize;

/// Registration error types with recovery suggestions
#[derive(Debug, Clone)]
pub enum RegistrationError {
    /// Node ID not found in Command Center
    InvalidNodeId { node_id: String },
    /// API Key is invalid or doesn't match the node
    InvalidApiKey { node_id: String },
    /// Network connectivity issue
    NetworkError { message: String },
    /// Server returned an error
    ServerError { code: u16, message: String },
    /// Camera codec detection failed
    CodecDetectionFailed { message: String },
    /// Configuration file missing or invalid
    ConfigError { message: String },
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNodeId { node_id } => write!(
                f,
                "Node ID '{}' not found in Command Center\n  \n  What to do:\n  → Open Command Center in your browser\n  → Go to Settings → Nodes\n  → Click 'Add Node' to create a new node\n  → Copy the Node ID and run setup again",
                node_id
            ),
            Self::InvalidApiKey { node_id } => write!(
                f,
                "Invalid API Key for node '{}'\n  \n  What to do:\n  → Open Command Center in your browser\n  → Go to Settings → Nodes\n  → Find your node and copy the API Key\n  → Make sure you copied the entire key (UUID format)",
                node_id
            ),
            Self::NetworkError { message } => write!(
                f,
                "Cannot reach Command Center\n  \n  Error: {}\n  \n  What to do:\n  → Check your internet connection\n  → Verify the API URL is correct\n  → Try again in a moment",
                message.lines().next().unwrap_or("Unknown network error")
            ),
            Self::ServerError { code, message } => write!(
                f,
                "Server error (HTTP {}): {}\n  \n  What to do:\n  → Wait a moment and try again\n  → If this persists, check Command Center logs\n  → Contact support if needed",
                code,
                message.lines().next().unwrap_or("Unknown server error")
            ),
            Self::CodecDetectionFailed { message } => write!(
                f,
                "Failed to detect camera codec\n  \n  Error: {}\n  \n  Using default codec: avc1.42e01e (H.264 Baseline)\n  This is safe and will work with most cameras.",
                message.lines().next().unwrap_or("Unknown error")
            ),
            Self::ConfigError { message } => write!(
                f,
                "Configuration error\n  \n  {}\n  \n  What to do:\n  → Run 'opensentry-cloudnode setup' to create config",
                message
            ),
        }
    }
}

/// Show a formatted error box with recovery instructions
pub fn show_registration_error(error: &RegistrationError) -> Result<()> {
    println!();
    println!(
        "{}",
        "╔════════════════════════════════════════════════════╗".red()
    );
    println!(
        "{}",
        "║            ⚠  Registration Failed                   ║".red()
    );
    println!(
        "{}",
        "╚════════════════════════════════════════════════════╝".red()
    );
    println!();

    // Show error message with proper indentation
    for line in error.to_string().lines() {
        if line.starts_with("  ") {
            println!("{}", line);
        } else {
            println!("  {}", line);
        }
    }

    println!();
    println!(
        "{}",
        "┌────────────────────────────────────────────────────┐".cyan()
    );
    println!(
        "{}",
        "│  Need Help?                                        │".cyan()
    );
    println!(
        "{}",
        "│  → Run: opensentry-cloudnode setup                 │".cyan()
    );
    println!(
        "{}",
        "│  → Config is stored in data/node.db                  │".cyan()
    );
    println!(
        "{}",
        "└────────────────────────────────────────────────────┘".cyan()
    );
    println!();

    Ok(())
}

/// Show a warning box (non-fatal errors)
pub fn show_warning(title: &str, message: &str) {
    println!();
    println!(
        "{}",
        "┌────────────────────────────────────────────────────┐".yellow()
    );
    println!(
        "{}{:width$}{}",
        "│ ".yellow(),
        title.yellow().bold(),
        "│".yellow(),
        width = 51 - title.len()
    );
    println!(
        "{}",
        "├────────────────────────────────────────────────────┤".yellow()
    );

    for line in message.lines() {
        println!(
            "{}{:width$}{}",
            "│ ".yellow(),
            line,
            "│".yellow(),
            width = 51 - line.len()
        );
    }

    println!(
        "{}",
        "└────────────────────────────────────────────────────┘".yellow()
    );
    println!();
}

/// Show a success box
pub fn show_success(title: &str, message: &str) {
    println!();
    println!(
        "{}",
        "┌────────────────────────────────────────────────────┐".green()
    );
    println!(
        "{}{:width$}{}",
        "│ ".green(),
        title.green().bold(),
        "│".green(),
        width = 51 - title.len()
    );
    println!(
        "{}",
        "├────────────────────────────────────────────────────┤".green()
    );

    for line in message.lines() {
        println!(
            "{}{:width$}{}",
            "│ ".green(),
            line,
            "│".green(),
            width = 51 - line.len()
        );
    }

    println!(
        "{}",
        "└────────────────────────────────────────────────────┘".green()
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_formatting() {
        let error = RegistrationError::InvalidNodeId {
            node_id: "abc123".to_string(),
        };

        let output = error.to_string();
        assert!(output.contains("abc123"));
        assert!(output.contains("not found"));
    }
}
