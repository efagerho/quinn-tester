use std::sync::Arc;

use anyhow::Result;
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};

const SERVER_ADDR: &str = "127.0.0.1:5000";
const NUM_PINGS: usize = 10;
const PING: &[u8] = b"ping";
const PONG: &[u8] = b"pong";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("server") => run_server().await,
        Some("client") => run_client().await,
        _ => {
            eprintln!("Usage: quinn-tester [server|client]");
            Ok(())
        }
    }
}

async fn run_server() -> Result<()> {
    let server_config =
        ServerConfig::with_crypto(Arc::new(quinn_plaintext::PlaintextServerConfig));
    let endpoint = Endpoint::server(server_config, SERVER_ADDR.parse()?)?;
    println!("Server listening on {SERVER_ADDR}");

    while let Some(incoming) = endpoint.accept().await {
        let conn = incoming.await?;
        tokio::spawn(async move {
            if let Err(e) = handle_connection(conn).await {
                eprintln!("Connection error: {e}");
            }
        });
    }

    Ok(())
}

async fn handle_connection(conn: Connection) -> Result<()> {
    println!("New connection from {}", conn.remote_address());

    loop {
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
            Err(e) => return Err(e.into()),
        };

        let mut buf = [0u8; 4];
        recv.read_exact(&mut buf).await?;
        assert_eq!(&buf, PING, "Expected 'ping'");
        println!("Server: received ping");

        send.write_all(PONG).await?;
        send.finish()?;
        println!("Server: sent pong");
    }

    println!("Connection closed");
    Ok(())
}

async fn run_client() -> Result<()> {
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(ClientConfig::new(Arc::new(
        quinn_plaintext::PlaintextClientConfig,
    )));

    let conn = endpoint.connect(SERVER_ADDR.parse()?, "localhost")?.await?;
    println!("Connected to {SERVER_ADDR}");

    for i in 1..=NUM_PINGS {
        let (mut send, mut recv) = conn.open_bi().await?;

        send.write_all(PING).await?;
        send.finish()?;
        println!("Client: sent ping #{i}");

        let mut buf = [0u8; 4];
        recv.read_exact(&mut buf).await?;
        assert_eq!(&buf, PONG, "Expected 'pong'");
        println!("Client: received pong #{i}");
    }

    conn.close(0u32.into(), b"done");
    endpoint.wait_idle().await;

    println!("Done — {NUM_PINGS} ping-pong exchanges completed");
    Ok(())
}
