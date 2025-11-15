use std::fs::File as StdFile;
use std::io::BufReader;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use quinn::rustls::{
    self,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use quinn::{crypto, ClientConfig, Endpoint};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::config::{DiscoveryConfig, TlsConfig};

/// Active server endpoint, including client configuration for outbound connections.
pub struct ServerContext {
    pub endpoint: Endpoint,
    pub client_config: ClientConfig,
}

/// Standalone client endpoint used for CLI commands.
pub struct ClientContext {
    pub endpoint: Endpoint,
    pub client_config: ClientConfig,
}

/// Creates a QUIC server endpoint.
pub fn bind_server(bind_addr: SocketAddr, tls: &TlsConfig) -> Result<ServerContext> {
    let tls_material = prepare_server_cert(tls)?;
    let client_config = build_client_config(tls, Some(&tls_material.certs))?;

    let mut server_config = quinn::ServerConfig::with_single_cert(
        tls_material.certs.clone(),
        tls_material.key.clone_key(),
    )
    .context("failed to build server config")?;
    server_config.transport = default_transport_config();

    let mut endpoint =
        Endpoint::server(server_config, bind_addr).context("failed to bind QUIC server")?;
    endpoint.set_default_client_config(client_config.clone());

    Ok(ServerContext {
        endpoint,
        client_config,
    })
}

/// Creates a QUIC client endpoint suitable for dialing peers.
pub fn build_client_endpoint(bind_addr: SocketAddr, tls: &TlsConfig) -> Result<ClientContext> {
    let client_config = build_client_config(tls, None)?;
    let mut endpoint =
        Endpoint::client(bind_addr).context("failed to open local client endpoint")?;
    endpoint.set_default_client_config(client_config.clone());
    Ok(ClientContext {
        endpoint,
        client_config,
    })
}

/// Establishes a QUIC connection to `peer`.
pub async fn connect(
    endpoint: &Endpoint,
    client_config: &ClientConfig,
    peer: SocketAddr,
    server_name: &str,
) -> Result<quinn::Connection> {
    let connection = endpoint
        .connect_with(client_config.clone(), peer, server_name)
        .context("failed to start QUIC handshake")?
        .await
        .context("failed to establish QUIC connection")?;
    Ok(connection)
}

/// Spawns a UDP discovery responder that answers broadcast probes.
pub async fn spawn_discovery_responder(
    bind_addr: SocketAddr,
    discovery: &DiscoveryConfig,
) -> Result<Option<JoinHandle<()>>> {
    if !discovery.enabled {
        return Ok(None);
    }

    let socket = UdpSocket::bind((IpAddr::V4(Ipv4Addr::UNSPECIFIED), discovery.port))
        .await
        .context("failed to bind discovery socket")?;
    let magic = discovery.magic.clone();
    let port = bind_addr.port();

    let handle = tokio::spawn(async move {
        let mut buf = vec![0u8; 256];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let payload = &buf[..len];
                    if payload.starts_with(magic.as_bytes()) {
                        let reply = format!("{magic}:{port}");
                        if let Err(err) = socket.send_to(reply.as_bytes(), addr).await {
                            tracing::warn!(%err, "failed to reply to discovery probe");
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, "discovery responder exiting");
                    break;
                }
            }
        }
    });

    Ok(Some(handle))
}

/// Broadcast-based peer discovery helper.
pub async fn discover_peers(discovery: &DiscoveryConfig) -> Result<Vec<SocketAddr>> {
    if !discovery.enabled {
        return Ok(Vec::new());
    }

    let socket = UdpSocket::bind((IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))
        .await
        .context("failed to bind discovery client socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable UDP broadcast")?;

    let target = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), discovery.port);
    socket
        .send_to(discovery.magic.as_bytes(), target)
        .await
        .context("failed to send discovery probe")?;

    let mut peers = Vec::new();
    let mut buf = vec![0u8; 256];
    let deadline = tokio::time::Instant::now()
        + tokio::time::Duration::from_millis(discovery.response_ttl_ms.max(100));

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, addr))) => {
                if let Some(peer) = parse_discovery_reply(&buf[..len], addr.ip()) {
                    peers.push(peer);
                }
            }
            Ok(Err(err)) => {
                tracing::warn!(%err, "discovery recv failure");
                break;
            }
            Err(_) => break,
        }
    }

    peers.sort();
    peers.dedup();
    Ok(peers)
}

fn parse_discovery_reply(payload: &[u8], ip: IpAddr) -> Option<SocketAddr> {
    let text = std::str::from_utf8(payload).ok()?;
    let (_, port_str) = text.split_once(':')?;
    let port = port_str.parse().ok()?;
    Some(SocketAddr::new(ip, port))
}

fn default_transport_config() -> Arc<quinn::TransportConfig> {
    let mut transport_config = quinn::TransportConfig::default();
    transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    transport_config.max_concurrent_uni_streams(quinn::VarInt::from_u32(256));
    transport_config.max_concurrent_bidi_streams(quinn::VarInt::from_u32(32));
    Arc::new(transport_config)
}

fn build_client_config(
    tls: &TlsConfig,
    peer_certs: Option<&[CertificateDer<'static>]>,
) -> Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    if let Some(certs) = peer_certs {
        for cert in certs {
            roots.add(cert.clone()).context("failed to add peer cert")?;
        }
    } else if let Some(cert_path) = &tls.cert_path {
        for cert in read_certs(cert_path)? {
            roots.add(cert).with_context(|| {
                format!("failed loading trust store from {}", cert_path.display())
            })?;
        }
    }

    let builder = rustls::ClientConfig::builder();
    let builder = if tls.insecure_local {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
    } else {
        builder.with_root_certificates(Arc::new(roots))
    };
    let mut rustls_config = builder.with_no_client_auth();
    rustls_config.alpn_protocols = vec![b"dezap/1".to_vec()];
    if tls.insecure_local {
        rustls_config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoVerifier));
    }

    let crypto = crypto::rustls::QuicClientConfig::try_from(Arc::new(rustls_config))
        .context("failed to convert rustls client config")?;
    let mut client_config = ClientConfig::new(Arc::new(crypto));
    client_config.transport_config(default_transport_config());
    Ok(client_config)
}

struct TlsMaterial {
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

fn prepare_server_cert(tls: &TlsConfig) -> Result<TlsMaterial> {
    if let (Some(cert_path), Some(key_path)) = (&tls.cert_path, &tls.key_path) {
        return load_from_disk(cert_path, key_path);
    }
    generate_self_signed(tls)
}

fn load_from_disk(cert_path: &Path, key_path: &Path) -> Result<TlsMaterial> {
    let certs = read_certs(cert_path)?;
    let key = read_private_key(key_path)?;
    Ok(TlsMaterial { certs, key })
}

fn read_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = StdFile::open(path)
        .with_context(|| format!("unable to open certificate {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .context("failed to parse certificate")?;
    Ok(certs)
}

fn read_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = StdFile::open(path)
        .with_context(|| format!("unable to open private key {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let key = rustls_pemfile::private_key(&mut reader)?.context("no private key entries found")?;
    Ok(key)
}

fn generate_self_signed(tls: &TlsConfig) -> Result<TlsMaterial> {
    let mut params = CertificateParams::new(vec![tls.server_name.clone(), "localhost".into()])
        .context("failed to build certificate params")?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, tls.server_name.clone());
    params
        .subject_alt_names
        .push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    params
        .subject_alt_names
        .push(SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)));

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    let cert_der = cert.der().clone();
    let key_der = PrivatePkcs8KeyDer::from(key_pair.serialize_der());

    Ok(TlsMaterial {
        certs: vec![cert_der],
        key: key_der.into(),
    })
}

#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
        ]
    }
}
