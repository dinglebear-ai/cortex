use super::*;

#[test]
fn parses_local_tailscale_identity() {
    let mut out = CollectorOutput::new("tailscale");
    parse_status(
        r#"{"Self":{"HostName":"devhost","OS":"linux","TailscaleIPs":["198.51.100.6"]}}"#,
        &mut out,
    );
    assert_eq!(out.nodes[0].hostname, "devhost");
    assert_eq!(out.nodes[0].ips, vec!["198.51.100.6"]);
}
