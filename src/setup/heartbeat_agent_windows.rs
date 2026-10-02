//! Windows user task lifecycle; uses the same agent env and managed binary.
use super::{PhaseTimer, SetupPhase, SetupStatus};
use std::{io, path::Path};
const TASK: &str = "CortexHeartbeatAgent";
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
fn run(script: &str) -> io::Result<String> {
    let output = crate::env::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.contains("compensation failed:") {
            "Windows upgrade failed and compensation failed; previous binary retained beside the managed executable; inspect Task Scheduler"
        } else if stderr
            .contains("previous task and binary restored; previous environment restored")
        {
            "Windows upgrade failed; previous task and binary restored; previous environment restored"
        } else {
            "Windows heartbeat task operation failed; inspect Task Scheduler"
        };
        return Err(io::Error::other(detail));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}
fn registration_script(bin: &Path, env: &Path, host_id: &Path) -> String {
    let agent_script = format!(
        "$env:HOME=$env:USERPROFILE; & {} heartbeat agent --env-file {} --host-id-path {}; exit $LASTEXITCODE",
        ps_quote(&bin.display().to_string()),
        ps_quote(&env.display().to_string()),
        ps_quote(&host_id.display().to_string())
    );
    format!(
        "$ErrorActionPreference='Stop'; $agentScript={}; $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($agentScript)); $a=New-ScheduledTaskAction -Execute (Join-Path $PSHOME 'powershell.exe') -Argument ('-NoProfile -NonInteractive -EncodedCommand '+$encoded); $t=New-ScheduledTaskTrigger -AtLogOn -User ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name); $p=New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive; $s=New-ScheduledTaskSettingsSet -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero); Register-ScheduledTask -TaskName '{TASK}' -Action $a -Trigger $t -Principal $p -Settings $s -Force | Out-Null; Start-ScheduledTask -TaskName '{TASK}'; Start-Sleep -Seconds 2; if ((Get-ScheduledTask -TaskName '{TASK}').State -ne 'Running') {{throw 'Task did not start'}}",
        ps_quote(&agent_script)
    )
}

pub(super) fn upgrade(
    staged: &Path,
    bin: &Path,
    staged_env: &Path,
    env: &Path,
    host_id: &Path,
) -> io::Result<Vec<SetupPhase>> {
    run(&upgrade_script(staged, bin, staged_env, env, host_id))?;
    Ok(vec![PhaseTimer::start("heartbeat-agent-service").finish(
        SetupStatus::Ok,
        "Windows user task running; previous binary retained; starts at user logon",
    )])
}

fn upgrade_script(
    staged: &Path,
    bin: &Path,
    staged_env: &Path,
    env: &Path,
    host_id: &Path,
) -> String {
    format!(
        r#"$ErrorActionPreference='Stop';
$candidate={candidate}; $bin={bin}; $backup=$bin+'.previous';
$candidateEnv={candidate_env}; $envPath={env_path}; $envBackup=$envPath+'.previous';
foreach ($file in @($candidate,$candidateEnv)) {{ if (!(Test-Path -LiteralPath $file -PathType Leaf)) {{ throw 'Staged upgrade file is missing' }}; if ((Get-Item -LiteralPath $file).Attributes -band [IO.FileAttributes]::ReparsePoint) {{ throw 'Staged upgrade file must not be a link' }} }};
# Candidate must run before touching the current task or executable.
& $candidate --version; if ($LASTEXITCODE -ne 0) {{ throw 'Candidate cannot run' }};
$oldTask=Get-ScheduledTask -TaskName '{TASK}' -ErrorAction SilentlyContinue;
$oldXml=$null; $wasRunning=$false;
if ($oldTask) {{
  $owner=$oldTask.Principal.UserId; $identity=[System.Security.Principal.WindowsIdentity]::GetCurrent();
  if ($owner -ne $identity.Name -and $owner -ne $identity.User.Value) {{ throw 'Existing task belongs to another user' }};
  $oldXml=Export-ScheduledTask -TaskName '{TASK}'; $wasRunning=($oldTask.State -eq 'Running') }};
$hadBinary=Test-Path -LiteralPath $bin;
if ($hadBinary) {{ if (!(Test-Path -LiteralPath $bin -PathType Leaf) -or ((Get-Item -LiteralPath $bin).Attributes -band [IO.FileAttributes]::ReparsePoint)) {{ throw 'Previous binary must be a regular file' }}; Copy-Item -LiteralPath $bin -Destination $backup -Force }};
$hadEnv=Test-Path -LiteralPath $envPath; if ($hadEnv) {{ Copy-Item -LiteralPath $envPath -Destination $envBackup -Force }};
$changed=$false;
try {{
  if ($oldTask) {{ Stop-ScheduledTask -TaskName '{TASK}'; $deadline=(Get-Date).AddSeconds(15); while ((Get-ScheduledTask -TaskName '{TASK}').State -eq 'Running') {{ if ((Get-Date) -gt $deadline) {{ throw 'Previous task did not stop' }}; Start-Sleep -Milliseconds 100 }} }};
  $changed=$true; Move-Item -LiteralPath $candidate -Destination $bin -Force;
  Move-Item -LiteralPath $candidateEnv -Destination $envPath -Force;
  {register}
}} catch {{
  $original=$_.Exception.Message;
  try {{
    $current=Get-ScheduledTask -TaskName '{TASK}' -ErrorAction SilentlyContinue;
    if ($current) {{ Stop-ScheduledTask -TaskName '{TASK}'; Start-Sleep -Seconds 2 }};
    if ($changed) {{ if ($hadBinary) {{ Copy-Item -LiteralPath $backup -Destination $bin -Force }} elseif (Test-Path $bin) {{ Remove-Item $bin }} }};
    if ($changed) {{ if ($hadEnv) {{ Copy-Item -LiteralPath $envBackup -Destination $envPath -Force }} elseif (Test-Path -LiteralPath $envPath) {{ Remove-Item -LiteralPath $envPath }} }};
    if ($oldXml) {{ Register-ScheduledTask -TaskName '{TASK}' -Xml $oldXml -Force | Out-Null; if ($wasRunning) {{ Start-ScheduledTask -TaskName '{TASK}'; Start-Sleep -Seconds 2; if ((Get-ScheduledTask -TaskName '{TASK}').State -ne 'Running') {{ throw 'Previous task did not restart' }} }} }}
    elseif ($current) {{ Unregister-ScheduledTask -TaskName '{TASK}' -Confirm:$false }};
  }} catch {{ throw ('Upgrade failed: '+$original+'; compensation failed: '+$_.Exception.Message+'; previous binary retained at '+$backup) }};
  throw ('Upgrade failed: '+$original+'; previous task and binary restored; previous environment restored');
}} finally {{ if (Test-Path $candidate) {{ Remove-Item $candidate -ErrorAction SilentlyContinue }} }};
"#,
        candidate = ps_quote(&staged.display().to_string()),
        bin = ps_quote(&bin.display().to_string()),
        candidate_env = ps_quote(&staged_env.display().to_string()),
        env_path = ps_quote(&env.display().to_string()),
        register = registration_script(bin, env, host_id)
    )
}

pub(super) fn remove() -> io::Result<()> {
    run(&format!(
        "$ErrorActionPreference='Stop'; $t=Get-ScheduledTask -TaskName '{TASK}' -ErrorAction SilentlyContinue; if ($t) {{ Stop-ScheduledTask -TaskName '{TASK}'; Unregister-ScheduledTask -TaskName '{TASK}' -Confirm:$false }}"
    ))?;
    Ok(())
}
pub(super) fn check() -> SetupPhase {
    let result = run(&format!(
        "$ErrorActionPreference='Stop'; (Get-ScheduledTask -TaskName '{TASK}').State"
    ));
    match result {
        Ok(state) if state == "Running" => PhaseTimer::start("heartbeat-agent-service")
            .finish(SetupStatus::Ok, "Windows task running"),
        _ => PhaseTimer::start("heartbeat-agent-service").finish(
            SetupStatus::Error,
            "Windows task is absent or inactive; run cortex setup heartbeatagent install",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upgrade_validates_before_stop_and_compensates_task_registration_failure() {
        let script = upgrade_script(
            Path::new("candidate.exe"),
            Path::new("managed.exe"),
            Path::new("candidate.env"),
            Path::new("agent.env"),
            Path::new("host-id"),
        );
        let validation = script.find("& $candidate --version").unwrap();
        let stop = script.find("Stop-ScheduledTask").unwrap();
        let replace = script
            .find("Move-Item -LiteralPath $candidate -Destination $bin")
            .unwrap();
        let registration = script.find("$agentScript=").unwrap();
        assert!(validation < stop && stop < replace && replace < registration);
        assert!(script.contains("$oldXml=Export-ScheduledTask"));
        assert!(
            script.contains("Register-ScheduledTask -TaskName 'CortexHeartbeatAgent' -Xml $oldXml")
        );
        assert!(script.contains("if ($wasRunning) { Start-ScheduledTask"));
        assert!(script.contains("Copy-Item -LiteralPath $backup -Destination $bin -Force"));
        assert!(script.contains("compensation failed:"));
        let env_commit = script
            .find("Move-Item -LiteralPath $candidateEnv -Destination $envPath")
            .unwrap();
        let env_restore = script
            .find("Copy-Item -LiteralPath $envBackup -Destination $envPath")
            .unwrap();
        let old_restart = script
            .find("if ($wasRunning) { Start-ScheduledTask")
            .unwrap();
        assert!(replace < env_commit && env_commit < registration);
        assert!(env_restore < old_restart);
        assert!(script.contains("Existing task belongs to another user"));
    }
}
