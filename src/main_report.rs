//! Human-readable update command reporting.

pub(super) fn print_update_report(report: &cortex::update::UpdateReport) {
    println!("cortex update {}", report.mode);
    println!("profile: {}", report.profile_path.display());
    if let Some(server) = &report.server {
        println!("server: {} {}", server.host, server.home);
        for phase in &server.phases {
            println!(
                "server\t{:?}\t{}\t{}ms\t{}",
                phase.status, phase.name, phase.elapsed_ms, phase.detail
            );
        }
    }
    for client in &report.clients {
        println!(
            "client\t{}\t{}\t{}ms\t{}",
            if client.ok { "ok" } else { "error" },
            client.host,
            client.elapsed_ms,
            client.detail
        );
    }
    for phase in &report.skipped {
        println!(
            "skip\t{:?}\t{}\t{}ms\t{}",
            phase.status, phase.name, phase.elapsed_ms, phase.detail
        );
    }
}
