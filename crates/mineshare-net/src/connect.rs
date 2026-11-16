//! Bounded dialing across the addresses in one peer advertisement.
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::net::TcpStream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

pub async fn connect_peer(addresses: &[IpAddr], port: u16) -> Result<TcpStream> {
    let candidates = ordered_candidates(addresses, port);
    connect_candidates(candidates, CONNECT_TIMEOUT, TcpStream::connect).await
}

/// Follow endpoint changes only while opening TCP, never after authentication.
pub async fn connect_peer_refreshing(latest: impl Fn() -> (Vec<IpAddr>, u16)) -> Result<TcpStream> {
    connect_refreshing(
        || {
            let (addresses, port) = latest();
            ordered_candidates(&addresses, port)
        },
        CONNECT_TIMEOUT,
        TcpStream::connect,
    )
    .await
}

async fn connect_refreshing<T, F, Fut>(
    latest: impl Fn() -> Vec<SocketAddr>,
    timeout: Duration,
    connect: F,
) -> Result<T>
where
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = std::io::Result<T>>,
{
    tokio::time::timeout(timeout, async {
        let mut addresses = latest();
        loop {
            let attempt = connect_candidates(addresses.clone(), timeout, &connect);
            tokio::pin!(attempt);
            let mut refresh = tokio::time::interval(Duration::from_millis(50));
            refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    result = &mut attempt => return result,
                    _ = refresh.tick() => {
                        let updated = latest();
                        if updated != addresses {
                            addresses = updated;
                            break;
                        }
                    }
                }
            }
            // Drop the pending sockets before trying the newly discovered endpoint.
        }
    })
    .await
    .context("peer connection timed out while endpoint changed")?
}

fn ordered_candidates(addresses: &[IpAddr], port: u16) -> Vec<SocketAddr> {
    let mut candidates: Vec<_> = addresses
        .iter()
        .copied()
        .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
        .filter(|ip| !matches!(ip, IpAddr::V4(ip) if ip.is_broadcast()))
        .collect();
    // mDNS address order is arbitrary. Try the usual LAN route before
    // link-local addresses belonging to disconnected or virtual adapters.
    candidates.sort_unstable_by_key(|ip| {
        let rank = match ip {
            IpAddr::V4(ip) if !ip.is_link_local() => 0,
            IpAddr::V6(ip) if !ip.is_unicast_link_local() => 1,
            IpAddr::V4(_) => 2,
            IpAddr::V6(_) => 3,
        };
        (rank, *ip)
    });
    candidates.dedup();
    candidates
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect()
}

async fn connect_candidates<T, F, Fut>(
    addresses: Vec<SocketAddr>,
    timeout: Duration,
    connect: F,
) -> Result<T>
where
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = std::io::Result<T>>,
{
    anyhow::ensure!(!addresses.is_empty(), "peer has no usable addresses");
    let connect = &connect;
    let mut pending: FuturesUnordered<_> = addresses
        .into_iter()
        .enumerate()
        .map(|(index, address)| async move {
            // Give the preferred route a brief head start. Cap the stagger so
            // even a long advert gets a chance within one total deadline.
            if index > 0 {
                tokio::time::sleep(Duration::from_millis(index.min(3) as u64 * 100)).await;
            }
            (address, connect(address).await)
        })
        .collect();
    tokio::time::timeout(timeout, async {
        let mut last_error = None;
        while let Some((address, result)) = pending.next().await {
            match result {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = Some(format!("{address}: {error}")),
            }
        }
        anyhow::bail!(
            "all addresses failed (last: {})",
            last_error.unwrap_or_default()
        )
    })
    .await
    .context("peer connection timed out across all advertised addresses")?
    // Dropping FuturesUnordered cancels the losing connects and closes their
    // sockets. Nothing is spawned beyond this connection attempt's lifetime.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_lan_endpoint_interrupts_pending_stale_dial() {
        let stale: SocketAddr = "192.0.2.1:1234".parse().unwrap();
        let fresh: SocketAddr = "192.0.2.2:5678".parse().unwrap();
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            connect_refreshing(
                || {
                    if started.elapsed() < Duration::from_millis(80) {
                        vec![stale]
                    } else {
                        vec![fresh]
                    }
                },
                CONNECT_TIMEOUT,
                |address| async move {
                    if address == stale {
                        std::future::pending::<()>().await;
                    }
                    Ok(address)
                },
            ),
        )
        .await
        .expect("new LAN address must not wait for the old TCP timeout")
        .unwrap();
        assert_eq!(result, fresh);
    }

    #[tokio::test]
    async fn changing_endpoints_cannot_extend_total_deadline() {
        let revision = std::cell::Cell::new(1000);
        let started = std::time::Instant::now();
        let result = connect_refreshing(
            || {
                revision.set(revision.get() + 1);
                vec![SocketAddr::from(([192, 0, 2, 1], revision.get()))]
            },
            Duration::from_millis(160),
            |_| std::future::pending::<std::io::Result<()>>(),
        )
        .await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn unreachable_address_does_not_hold_back_live_lan_address() {
        let dead: SocketAddr = "192.0.2.1:1234".parse().unwrap();
        let live: SocketAddr = "192.0.2.2:1234".parse().unwrap();
        let connected = tokio::time::timeout(
            Duration::from_millis(500),
            connect_candidates(vec![dead, live], CONNECT_TIMEOUT, |address| async move {
                if address == dead {
                    std::future::pending::<()>().await;
                }
                Ok(address)
            }),
        )
        .await
        .expect("a dead address must not consume the live peer's connection budget")
        .unwrap();
        assert_eq!(connected, live);
    }

    #[test]
    fn multi_adapter_addresses_prefer_lan_and_deduplicate() {
        let addresses: Vec<IpAddr> = [
            "fe80::1",
            "169.254.83.107",
            "192.168.1.108",
            "fd00::1",
            "192.168.1.108",
            "0.0.0.0",
            "::",
            "224.0.0.251",
            "ff02::fb",
        ]
        .into_iter()
        .map(|ip| ip.parse().unwrap())
        .collect();
        let actual: Vec<_> = ordered_candidates(&addresses, 1234)
            .into_iter()
            .map(|address| address.ip().to_string())
            .collect();
        assert_eq!(
            actual,
            ["192.168.1.108", "fd00::1", "169.254.83.107", "fe80::1"]
        );
    }

    #[tokio::test]
    async fn unreachable_candidates_share_one_deadline() {
        let dead = "192.0.2.1:1234".parse().unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            connect_candidates(vec![dead; 8], Duration::from_millis(80), |_| {
                std::future::pending::<std::io::Result<()>>()
            }),
        )
        .await
        .expect("address count must not multiply the timeout");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("across all advertised addresses")
        );
    }

    #[tokio::test]
    async fn successful_connection_drops_pending_losers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Guard<'a>(&'a AtomicUsize);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = AtomicUsize::new(0);
        let dead: SocketAddr = "192.0.2.1:1234".parse().unwrap();
        let live: SocketAddr = "192.0.2.2:1234".parse().unwrap();
        connect_candidates(vec![dead, live], CONNECT_TIMEOUT, |address| {
            let dropped = &dropped;
            async move {
                if address == dead {
                    let _guard = Guard(dropped);
                    std::future::pending::<()>().await;
                }
                Ok(address)
            }
        })
        .await
        .unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn loopback_connection_uses_a_live_listener_after_a_stale_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = listener.local_addr().unwrap();
        let stale = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead = stale.local_addr().unwrap();
        drop(stale);
        let stream = connect_candidates(vec![dead, live], CONNECT_TIMEOUT, TcpStream::connect)
            .await
            .unwrap();
        assert_eq!(stream.peer_addr().unwrap(), live);
        let (_, peer) = listener.accept().await.unwrap();
        assert_eq!(stream.local_addr().unwrap(), peer);
    }
}
