//! Target-platform planning. Detection happens before any managed file is changed.
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Platform {
    LinuxX64,
    LinuxArm64,
    MacArm64,
    WindowsX64,
}
impl Platform {
    pub(super) fn parse(os: &str, arch: &str) -> io::Result<Self> {
        match (
            os.trim().to_ascii_lowercase().as_str(),
            arch.trim().to_ascii_lowercase().as_str(),
        ) {
            ("linux", "x86_64" | "amd64") => Ok(Self::LinuxX64),
            ("linux", "aarch64" | "arm64") => Ok(Self::LinuxArm64),
            ("darwin" | "macos", "aarch64" | "arm64") => Ok(Self::MacArm64),
            ("windows", "x86_64" | "amd64" | "x64") => Ok(Self::WindowsX64),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported agent target {os}/{arch}; no files changed"),
            )),
        }
    }
    pub(super) fn asset(self) -> &'static str {
        match self {
            Self::LinuxX64 => "cortex-linux-x86_64.tar.gz",
            Self::LinuxArm64 => "cortex-linux-aarch64.tar.gz",
            Self::MacArm64 => "cortex-macos-arm64",
            Self::WindowsX64 => "cortex-windows-x86_64.exe",
        }
    }
}

pub(super) fn detect(host: &str) -> io::Result<Platform> {
    let output = super::ssh_capture(host, "uname -s && uname -m");
    if let Ok(output) = output {
        let mut lines = output.lines();
        if let (Some(os), Some(arch)) = (lines.next(), lines.next()) {
            return Platform::parse(os, arch);
        }
    }
    let output = super::ssh_capture(
        host,
        "powershell -NoProfile -NonInteractive -Command \"Write-Output windows; Write-Output ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString())\"",
    )?;
    let mut lines = output.lines();
    Platform::parse(lines.next().unwrap_or(""), lines.next().unwrap_or(""))
}

pub(super) fn stage_posix(platform: Platform) -> String {
    let asset = platform.asset();
    let url = format!(
        "https://github.com/dinglebear-ai/cortex/releases/download/v{}/{asset}",
        env!("CARGO_PKG_VERSION")
    );
    let extract = match platform {
        Platform::LinuxX64 | Platform::LinuxArm64 => {
            "tar -xzf \"$stage/artifact\" -C \"$stage\" cortex; test -f \"$stage/cortex\" && test ! -L \"$stage/cortex\"; cp \"$stage/cortex\" ~/.local/bin/cortex.new"
        }
        _ => "cp \"$stage/artifact\" ~/.local/bin/cortex.new",
    };
    format!(
        r#"set -eu; command -v curl >/dev/null; mkdir -p ~/.local/bin; umask 077; stage=$(mktemp -d); trap 'rm -rf "$stage"' EXIT; curl --fail --location --proto '=https' --connect-timeout 10 --max-time 180 --max-filesize 134217728 '{url}' -o "$stage/artifact"; curl --fail --location --proto '=https' --connect-timeout 10 --max-time 30 --max-filesize 4096 '{url}.sha256' -o "$stage/checksum"; expected=$(awk 'NR == 1 {{ print $1 }}' "$stage/checksum"); test "${{#expected}}" = 64; case "$expected" in *[!0-9a-fA-F]*) exit 1;; esac; if command -v sha256sum >/dev/null; then actual=$(sha256sum "$stage/artifact" | awk '{{print $1}}'); else actual=$(shasum -a 256 "$stage/artifact" | awk '{{print $1}}'); fi; test "$expected" = "$actual"; {extract}; chmod +x ~/.local/bin/cortex.new; ~/.local/bin/cortex.new --version; if test -f ~/.local/bin/cortex; then cp -p ~/.local/bin/cortex ~/.local/bin/cortex.previous; fi; mv -f ~/.local/bin/cortex.new ~/.local/bin/cortex"#
    )
}

pub(super) fn windows_stage() -> String {
    let url = format!(
        "https://github.com/dinglebear-ai/cortex/releases/download/v{}/cortex-windows-x86_64.exe",
        env!("CARGO_PKG_VERSION")
    );
    format!(
        r#"powershell -NoProfile -NonInteractive -Command "$ErrorActionPreference='Stop'; $dir=Join-Path $HOME '.local/bin'; New-Item -ItemType Directory -Force $dir | Out-Null; $bin=Join-Path $dir 'cortex.exe'; $new=Join-Path $dir 'cortex.new.exe'; Invoke-WebRequest -Uri '{url}' -OutFile $new; $sum=(Invoke-WebRequest -Uri '{url}.sha256').Content.Trim().Split(' ')[0]; if ($sum -notmatch '^[a-fA-F0-9]{{64}}$' -or (Get-FileHash -Algorithm SHA256 $new).Hash -ne $sum) {{Remove-Item $new; throw 'Artifact checksum mismatch'}}; & $new --version; if ($LASTEXITCODE -ne 0) {{throw 'Artifact cannot run on target'}}; if (Test-Path $bin) {{ Copy-Item $bin ($bin+'.previous') -Force }}; Move-Item $new $bin -Force""#
    )
}

pub(super) fn service_preflight(platform: Platform) -> &'static str {
    match platform {
        Platform::LinuxX64 | Platform::LinuxArm64 => {
            "set -eu; command -v curl >/dev/null; command -v tar >/dev/null; if test -d /run/systemd/system; then systemctl --user show-environment >/dev/null; else docker compose version >/dev/null; fi"
        }
        Platform::MacArm64 => {
            "set -eu; command -v curl >/dev/null; command -v shasum >/dev/null; launchctl print gui/$(id -u) >/dev/null"
        }
        Platform::WindowsX64 => {
            "powershell -NoProfile -NonInteractive -Command \"$ErrorActionPreference='Stop'; Get-Command Register-ScheduledTask, Get-FileHash, Invoke-WebRequest | Out-Null\""
        }
    }
}

/// Encode fixed PowerShell commands so SSH's cmd.exe or PowerShell default shell
/// cannot expand the embedded script. Secrets are supplied separately on stdin.
pub(super) fn shell_safe_command(command: &str) -> std::borrow::Cow<'_, str> {
    let Some(script) = command
        .strip_prefix("powershell -NoProfile -NonInteractive -Command \"")
        .and_then(|s| s.strip_suffix('"'))
    else {
        return std::borrow::Cow::Borrowed(command);
    };
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        encoded.push(ALPHABET[(a >> 2) as usize] as char);
        encoded.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    std::borrow::Cow::Owned(format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {encoded}"
    ))
}
