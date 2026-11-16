//! Probe peer routing without sending a pairing request or modifying trust.
use std::net::IpAddr;
use std::time::Instant;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: route_probe PORT IP..."))?
        .parse()?;
    let addresses: Vec<IpAddr> = args.map(|ip| ip.parse()).collect::<Result<_, _>>()?;
    let started = Instant::now();
    let stream = mineshare_net::connect::connect_peer(&addresses, port).await?;
    println!(
        "{}",
        serde_json::json!({
            "connected_to": stream.peer_addr()?.to_string(),
            "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
            "candidate_count": addresses.len(),
            "pairing_requested": false,
        })
    );
    Ok(())
}
