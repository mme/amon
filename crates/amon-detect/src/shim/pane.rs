//! Stands in for herdr's `crate::pane`.
//!
//! Only one constant is reachable from the vendored detect engine, and only
//! from a test asserting that a PowerShell running herdr's shell integration is
//! still classified as a shell rather than an agent. Kept verbatim so that test
//! keeps testing what it was written to test.

pub const WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND: &str = r"if ($null -eq $global:__HerdrOriginalPrompt) { $global:__HerdrOriginalPrompt = $function:prompt; function global:prompt { $out = @(& $global:__HerdrOriginalPrompt) -join ' '; $loc = $ExecutionContext.SessionState.Path.CurrentLocation; if ($loc.Provider.Name -eq 'FileSystem') { $esc = [string][char]27; $out += $esc + ']9;9;' + $loc.ProviderPath + $esc + '\' }; $out } }";
