use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use bytes::{Buf, BytesMut};

use quinn_proto::{
    crypto::{self, CryptoError, HeaderKey},
    transport_parameters, ConnectionId, Side, TransportError,
};
use seahash::SeaHasher;
use tracing::{error, trace};

pub fn server_config() -> quinn_proto::ServerConfig {
    quinn_proto::ServerConfig::with_crypto(Arc::new(PlaintextServerConfig::new()))
}

pub fn client_config() -> quinn_proto::ClientConfig {
    quinn_proto::ClientConfig::new(Arc::new(PlaintextClientConfig::new()))
}

pub struct PlaintextHeaderKey {
    side: Side,
}

impl PlaintextHeaderKey {
    pub fn new(side: Side) -> Self {
        Self { side }
    }
}

impl HeaderKey for PlaintextHeaderKey {
    fn decrypt(&self, pn_offset: usize, packet: &mut [u8]) {
        trace!(side = ?self.side, "HeaderKey::decrypt pn_offset: {pn_offset}");
        trace!(side = ?self.side, "packet: {packet:?}");
    }

    fn encrypt(&self, pn_offset: usize, packet: &mut [u8]) {
        trace!(side = ?self.side, "HeaderKey::encrypt pn_offset: {pn_offset}");
        trace!(side = ?self.side, "packet: {packet:?}");
    }

    fn sample_size(&self) -> usize {
        trace!(side = ?self.side, "HeaderKey::sample_size");
        0
    }
}

pub struct PlaintextPacketKey {
    side: Side,
}

impl PlaintextPacketKey {
    fn new(side: Side) -> Self {
        Self { side }
    }
}

#[derive(Default)]
pub struct PlaintextClientConfig;

impl PlaintextClientConfig {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Default)]
pub struct PlaintextServerConfig;

impl PlaintextServerConfig {
    pub fn new() -> Self {
        Self
    }
}

fn crypto_keys(side: Side) -> crypto::Keys {
    crypto::Keys {
        header: crypto_header_keypair(side),
        packet: crypto_packet_keypair(side),
    }
}

fn crypto_header_keypair(side: Side) -> crypto::KeyPair<Box<dyn crypto::HeaderKey>> {
    crypto::KeyPair {
        local: Box::new(PlaintextHeaderKey::new(side)),
        remote: Box::new(PlaintextHeaderKey::new(side)),
    }
}

fn crypto_packet_keypair(side: Side) -> crypto::KeyPair<Box<dyn crypto::PacketKey>> {
    crypto::KeyPair {
        local: Box::new(PlaintextPacketKey::new(side)),
        remote: Box::new(PlaintextPacketKey::new(side)),
    }
}

pub struct PlaintextSession {
    side: Side,
    params: transport_parameters::TransportParameters,
    peer_params: Option<transport_parameters::TransportParameters>,
    wrote_transporter_params: bool,
    initial_keys: Option<crypto::Keys>,
    handshake_keys: Option<crypto::Keys>,
}

impl PlaintextSession {
    fn new(side: Side, params: transport_parameters::TransportParameters) -> Self {
        Self {
            side,
            params,
            peer_params: None,
            wrote_transporter_params: false,
            initial_keys: Some(crypto_keys(side)),
            handshake_keys: Some(crypto_keys(side)),
        }
    }
}

impl crypto::Session for PlaintextSession {
    // 0.12: dst_cid by value, not &ConnectionId
    fn initial_keys(&self, dst_cid: ConnectionId, _side: Side) -> crypto::Keys {
        trace!(side = ?self.side, "initial_keys dst_cid: {dst_cid}");
        crypto_keys(self.side)
    }

    fn handshake_data(&self) -> Option<Box<dyn std::any::Any>> {
        trace!(side = ?self.side, "handshake_data");
        self.peer_params
            .map(|tp| Box::new(tp) as Box<dyn std::any::Any>)
    }

    fn peer_identity(&self) -> Option<Box<dyn std::any::Any>> {
        trace!(side = ?self.side, "peer_identity");
        None
    }

    fn early_crypto(&self) -> Option<(Box<dyn crypto::HeaderKey>, Box<dyn crypto::PacketKey>)> {
        trace!(side = ?self.side, "early_crypto");
        None
    }

    fn early_data_accepted(&self) -> Option<bool> {
        trace!(side = ?self.side, "early_data_accepted");
        Some(false)
    }

    fn is_handshaking(&self) -> bool {
        trace!(side = ?self.side, "is_handshaking");
        self.peer_params.is_none()
            || !self.wrote_transporter_params
                && (self.initial_keys.is_some() || self.handshake_keys.is_some())
    }

    fn read_handshake(&mut self, mut buf: &[u8]) -> Result<bool, TransportError> {
        trace!(side = ?self.side, "read_handshake {buf:?}");

        if self.peer_params.is_none() {
            self.peer_params = Some(
                transport_parameters::TransportParameters::read(self.side, &mut buf)
                    .expect("could not read transport parameters"),
            );
        }
        Ok(true)
    }

    fn transport_parameters(
        &self,
    ) -> Result<Option<transport_parameters::TransportParameters>, TransportError> {
        trace!(side = ?self.side, "transport_parameters");
        Ok(self.peer_params)
    }

    fn write_handshake(&mut self, buf: &mut Vec<u8>) -> Option<crypto::Keys> {
        if self.side.is_client() && !self.wrote_transporter_params {
            self.params.write(buf);
            self.wrote_transporter_params = true;
            trace!("wrote data: {buf:?}");
        }

        trace!(side = ?self.side, "write_handshake");

        match self.initial_keys.take().or_else(|| {
            self.handshake_keys.take().map(|k| {
                if self.side.is_server() && !self.wrote_transporter_params {
                    self.params.write(buf);
                    self.wrote_transporter_params = true;
                    trace!("wrote data: {buf:?}");
                }
                trace!("taking handshake keys");
                k
            })
        }) {
            Some(k) => Some(k),
            None => {
                trace!("returning None");
                None
            }
        }
    }

    fn next_1rtt_keys(&mut self) -> Option<crypto::KeyPair<Box<dyn crypto::PacketKey>>> {
        trace!(side = ?self.side, "next_1rtt_keys");
        Some(crypto_packet_keypair(self.side))
    }

    // 0.12: orig_dst_cid by value, not &ConnectionId
    fn is_valid_retry(&self, orig_dst_cid: ConnectionId, _header: &[u8], _payload: &[u8]) -> bool {
        trace!(side = ?self.side, "is_valid_retry orig_dst_cid: {orig_dst_cid}");
        true
    }

    fn export_keying_material(
        &self,
        _output: &mut [u8],
        _label: &[u8],
        _context: &[u8],
    ) -> Result<(), crypto::ExportKeyingMaterialError> {
        trace!(side = ?self.side, "export_keying_material");
        Ok(())
    }
}

impl crypto::ClientConfig for PlaintextClientConfig {
    fn start_session(
        self: Arc<Self>,
        version: u32,
        server_name: &str,
        params: &transport_parameters::TransportParameters,
    ) -> Result<Box<dyn crypto::Session>, quinn_proto::ConnectError> {
        trace!("ClientConfig::start_session version: {version}, server_name: {server_name}");
        Ok(Box::new(PlaintextSession::new(Side::Client, *params)))
    }
}

impl crypto::ServerConfig for PlaintextServerConfig {
    // 0.12: dst_cid by value, not &ConnectionId
    fn initial_keys(
        &self,
        version: u32,
        dst_cid: ConnectionId,
    ) -> Result<crypto::Keys, crypto::UnsupportedVersion> {
        trace!("ServerConfig::initial_keys version: {version}, dst_cid: {dst_cid:?}");
        Ok(crypto_keys(Side::Server))
    }

    // 0.12: orig_dst_cid by value, not &ConnectionId
    fn retry_tag(&self, version: u32, orig_dst_cid: ConnectionId, _packet: &[u8]) -> [u8; 16] {
        trace!("ServerConfig::retry_tag version: {version}, orig_dst_cid: {orig_dst_cid:?}");
        [0u8; 16]
    }

    fn start_session(
        self: Arc<Self>,
        version: u32,
        params: &transport_parameters::TransportParameters,
    ) -> Box<dyn crypto::Session> {
        trace!("ServerConfig::start_session version: {version}");
        Box::new(PlaintextSession::new(Side::Server, *params))
    }
}

impl crypto::PacketKey for PlaintextPacketKey {
    fn encrypt(&self, packet: u64, buf: &mut [u8], header_len: usize) {
        trace!(side = ?self.side, "PacketKey::encrypt packet: {packet}, header_len: {header_len}");
        let (header, payload_tag) = buf.split_at_mut(header_len);
        let (payload, tag_storage) = payload_tag.split_at_mut(payload_tag.len() - self.tag_len());
        let mut hasher = SeaHasher::default();
        header.hash(&mut hasher);
        payload.hash(&mut hasher);
        let checksum = hasher.finish();
        tag_storage.copy_from_slice(&checksum.to_be_bytes());
    }

    fn decrypt(
        &self,
        packet: u64,
        header: &[u8],
        payload: &mut BytesMut,
    ) -> Result<(), CryptoError> {
        trace!(side = ?self.side, "PacketKey::decrypt packet: {packet}");

        let mut tag_storage = payload.split_off(payload.len() - self.tag_len());

        let mut hasher = SeaHasher::default();
        header.hash(&mut hasher);
        payload.hash(&mut hasher);
        let checksum = hasher.finish();

        let expected = tag_storage.get_u64();
        if checksum != expected {
            error!(side = ?self.side, "checksum mismatch, expected {expected}, got: {checksum}");
            return Err(CryptoError);
        }

        Ok(())
    }

    fn tag_len(&self) -> usize {
        8
    }

    fn confidentiality_limit(&self) -> u64 {
        u64::MAX
    }

    fn integrity_limit(&self) -> u64 {
        1 << 36
    }
}
