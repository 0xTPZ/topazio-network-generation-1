use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::Duration,
};

use crate::{crypto::Hash, genesis::genesis_hash};

// Version 2 is required because transaction payloads now contain fees.
// The network identity itself remains unchanged.
pub const P2P_PROTOCOL_VERSION: u16 = 2;

pub const P2P_NETWORK_ID: &str = "topazio-v1-testnet";

const HANDSHAKE_MAGIC: &[u8; 8] = b"TPZP2P01";

const MAX_NETWORK_ID_LENGTH: usize = 64;

const MAX_NODE_ID_LENGTH: usize = 256;

const MAX_HANDSHAKE_FRAME_SIZE: usize = 4096;

const SOCKET_TIMEOUT_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub protocol_version: u16,
    pub network_id: String,
    pub node_id: String,
    pub height: u64,
    pub tip_hash: Hash,
    pub genesis_hash: Hash,
}

impl Handshake {
    pub fn new(node_id: String, height: u64, tip_hash: Hash) -> Result<Self, String> {
        validate_node_id(&node_id)?;

        Ok(Self {
            protocol_version: P2P_PROTOCOL_VERSION,

            network_id: P2P_NETWORK_ID.to_string(),

            node_id,

            height,

            tip_hash,

            genesis_hash: genesis_hash(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != P2P_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported P2P protocol version: {}",
                self.protocol_version
            ));
        }

        if self.network_id != P2P_NETWORK_ID {
            return Err(format!("wrong network id: {}", self.network_id));
        }

        validate_node_id(&self.node_id)?;

        if self.genesis_hash != genesis_hash() {
            return Err("genesis hash mismatch".to_string());
        }

        Ok(())
    }

    pub fn validate_peer(&self, peer: &Handshake) -> Result<(), String> {
        self.validate()?;
        peer.validate()?;

        if self.network_id != peer.network_id {
            return Err("peer belongs to a different network".to_string());
        }

        if self.protocol_version != peer.protocol_version {
            return Err("peer uses an incompatible protocol version".to_string());
        }

        if self.genesis_hash != peer.genesis_hash {
            return Err("peer genesis does not match local genesis".to_string());
        }

        if self.node_id == peer.node_id {
            return Err("peer has the same node id as local node".to_string());
        }

        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;

        let network_bytes = self.network_id.as_bytes();

        let node_bytes = self.node_id.as_bytes();

        if network_bytes.len() > u16::MAX as usize {
            return Err("network id is too long to encode".to_string());
        }

        if node_bytes.len() > u16::MAX as usize {
            return Err("node id is too long to encode".to_string());
        }

        let mut bytes = Vec::new();

        bytes.extend_from_slice(HANDSHAKE_MAGIC);

        bytes.extend_from_slice(&self.protocol_version.to_le_bytes());

        bytes.extend_from_slice(&(network_bytes.len() as u16).to_le_bytes());

        bytes.extend_from_slice(network_bytes);

        bytes.extend_from_slice(&(node_bytes.len() as u16).to_le_bytes());

        bytes.extend_from_slice(node_bytes);

        bytes.extend_from_slice(&self.height.to_le_bytes());

        bytes.extend_from_slice(&self.tip_hash);

        bytes.extend_from_slice(&self.genesis_hash);

        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut cursor = Cursor::new(bytes);

        let magic = cursor.read_exact(HANDSHAKE_MAGIC.len())?;

        if magic != HANDSHAKE_MAGIC {
            return Err("invalid Topazio P2P handshake magic".to_string());
        }

        let protocol_version = cursor.read_u16()?;

        let network_length = cursor.read_u16()? as usize;

        if network_length > MAX_NETWORK_ID_LENGTH {
            return Err("network id exceeds maximum length".to_string());
        }

        let network_id = cursor.read_string(network_length)?;

        let node_length = cursor.read_u16()? as usize;

        if node_length > MAX_NODE_ID_LENGTH {
            return Err("node id exceeds maximum length".to_string());
        }

        let node_id = cursor.read_string(node_length)?;

        let height = cursor.read_u64()?;

        let tip_hash = cursor.read_hash()?;

        let remote_genesis_hash = cursor.read_hash()?;

        if !cursor.is_finished() {
            return Err("handshake contains trailing bytes".to_string());
        }

        let handshake = Self {
            protocol_version,
            network_id,
            node_id,
            height,
            tip_hash,
            genesis_hash: remote_genesis_hash,
        };

        handshake.validate()?;

        Ok(handshake)
    }
}

/*
 * Envia um handshake utilizando um frame:
 *
 * [ tamanho u32 ][ handshake ]
 *
 * O prefixo de tamanho será reaproveitado
 * posteriormente pelo protocolo de mensagens
 * da Topazio.
 */
pub fn send_handshake(stream: &mut TcpStream, handshake: &Handshake) -> Result<(), String> {
    let payload = handshake.encode()?;

    if payload.len() > MAX_HANDSHAKE_FRAME_SIZE {
        return Err("handshake frame exceeds maximum size".to_string());
    }

    let payload_length =
        u32::try_from(payload.len()).map_err(|_| "handshake frame is too large".to_string())?;

    stream
        .write_all(&payload_length.to_le_bytes())
        .map_err(|error| format!("failed to write handshake frame length: {}", error))?;

    stream
        .write_all(&payload)
        .map_err(|error| format!("failed to write handshake payload: {}", error))?;

    stream
        .flush()
        .map_err(|error| format!("failed to flush handshake: {}", error))?;

    Ok(())
}

/*
 * Lê um frame da conexão sem confiar no
 * tamanho informado pelo outro computador.
 *
 * Isso evita reservar memória ilimitada
 * caso um peer malicioso mande um tamanho
 * absurdo.
 */
pub fn receive_handshake(stream: &mut TcpStream) -> Result<Handshake, String> {
    let mut length_bytes = [0u8; 4];

    stream
        .read_exact(&mut length_bytes)
        .map_err(|error| format!("failed to read handshake frame length: {}", error))?;

    let payload_length = u32::from_le_bytes(length_bytes) as usize;

    if payload_length == 0 {
        return Err("empty handshake frame".to_string());
    }

    if payload_length > MAX_HANDSHAKE_FRAME_SIZE {
        return Err(format!(
            "handshake frame exceeds maximum size: {}",
            payload_length
        ));
    }

    let mut payload = vec![0u8; payload_length];

    stream
        .read_exact(&mut payload)
        .map_err(|error| format!("failed to read handshake payload: {}", error))?;

    Handshake::decode(&payload)
}

/*
 * Configuração básica da conexão.
 *
 * Os timeouts impedem que um peer conecte
 * e deixe o nó preso indefinidamente sem
 * enviar dados.
 */
fn configure_stream(stream: &TcpStream) -> Result<(), String> {
    let timeout = Some(Duration::from_secs(SOCKET_TIMEOUT_SECONDS));

    stream
        .set_read_timeout(timeout)
        .map_err(|error| format!("failed to configure peer read timeout: {}", error))?;

    stream
        .set_write_timeout(timeout)
        .map_err(|error| format!("failed to configure peer write timeout: {}", error))?;

    stream
        .set_nodelay(true)
        .map_err(|error| format!("failed to configure TCP_NODELAY: {}", error))?;

    Ok(())
}

/*
 * Lado servidor:
 *
 * 1. aceita uma conexão;
 * 2. recebe handshake remoto;
 * 3. valida;
 * 4. envia handshake local.
 *
 * Retorna o handshake do peer aceito.
 */
pub fn accept_one_peer(
    listener: &TcpListener,
    local_handshake: &Handshake,
) -> Result<Handshake, String> {
    local_handshake.validate()?;

    let (mut stream, _peer_address) = listener
        .accept()
        .map_err(|error| format!("failed to accept peer connection: {}", error))?;

    configure_stream(&stream)?;

    let peer_handshake = receive_handshake(&mut stream)?;

    local_handshake.validate_peer(&peer_handshake)?;

    send_handshake(&mut stream, local_handshake)?;

    Ok(peer_handshake)
}

/*
 * Lado cliente:
 *
 * 1. conecta ao peer;
 * 2. envia handshake local;
 * 3. recebe handshake remoto;
 * 4. valida.
 */
pub fn connect_to_peer(address: &str, local_handshake: &Handshake) -> Result<Handshake, String> {
    local_handshake.validate()?;

    let mut stream = TcpStream::connect(address)
        .map_err(|error| format!("failed to connect to peer {}: {}", address, error))?;

    configure_stream(&stream)?;

    send_handshake(&mut stream, local_handshake)?;

    let peer_handshake = receive_handshake(&mut stream)?;

    local_handshake.validate_peer(&peer_handshake)?;

    Ok(peer_handshake)
}

fn validate_node_id(node_id: &str) -> Result<(), String> {
    if node_id.is_empty() {
        return Err("node id cannot be empty".to_string());
    }

    if node_id.len() > MAX_NODE_ID_LENGTH {
        return Err("node id exceeds maximum length".to_string());
    }

    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| "handshake cursor overflow".to_string())?;

        if end > self.bytes.len() {
            return Err("handshake is truncated".to_string());
        }

        let slice = &self.bytes[self.position..end];

        self.position = end;

        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let bytes = self.read_exact(2)?;

        let mut value = [0u8; 2];

        value.copy_from_slice(bytes);

        Ok(u16::from_le_bytes(value))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        let bytes = self.read_exact(8)?;

        let mut value = [0u8; 8];

        value.copy_from_slice(bytes);

        Ok(u64::from_le_bytes(value))
    }

    fn read_hash(&mut self) -> Result<Hash, String> {
        let bytes = self.read_exact(32)?;

        let mut hash = [0u8; 32];

        hash.copy_from_slice(bytes);

        Ok(hash)
    }

    fn read_string(&mut self, length: usize) -> Result<String, String> {
        let bytes = self.read_exact(length)?;

        String::from_utf8(bytes.to_vec())
            .map_err(|error| format!("invalid UTF-8 in handshake string: {}", error))
    }

    fn is_finished(&self) -> bool {
        self.position == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        io::Write,
        net::{TcpListener, TcpStream},
        thread,
    };

    use crate::{crypto::sha256, identity::KeyPair};

    fn node_a_id() -> String {
        KeyPair::from_secret_bytes([7u8; 32]).address()
    }

    fn node_b_id() -> String {
        KeyPair::from_secret_bytes([9u8; 32]).address()
    }

    fn sample_tip_hash() -> Hash {
        sha256(b"topazio-test-tip")
    }

    #[test]
    fn handshake_uses_expected_protocol_version() {
        let handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        assert_eq!(handshake.protocol_version, P2P_PROTOCOL_VERSION);
    }

    #[test]
    fn handshake_uses_expected_network_id() {
        let handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        assert_eq!(handshake.network_id, P2P_NETWORK_ID);
    }

    #[test]
    fn handshake_contains_current_genesis_hash() {
        let handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        assert_eq!(handshake.genesis_hash, genesis_hash());
    }

    #[test]
    fn handshake_encode_decode_round_trip() {
        let original = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let encoded = original.encode().unwrap();

        let decoded = Handshake::decode(&encoded).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn wrong_network_is_rejected() {
        let mut handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        handshake.network_id = "another-network".to_string();

        assert!(handshake.validate().is_err());
    }

    #[test]
    fn wrong_genesis_is_rejected() {
        let mut handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        handshake.genesis_hash = [1u8; 32];

        assert!(handshake.validate().is_err());
    }

    #[test]
    fn truncated_handshake_is_rejected() {
        let handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let encoded = handshake.encode().unwrap();

        let truncated = &encoded[..encoded.len() - 1];

        assert!(Handshake::decode(truncated,).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let mut encoded = handshake.encode().unwrap();

        encoded.push(99);

        assert!(Handshake::decode(&encoded,).is_err());
    }

    #[test]
    fn different_valid_nodes_can_handshake() {
        let local = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let peer = Handshake::new(node_b_id(), 10, sample_tip_hash()).unwrap();

        assert!(local.validate_peer(&peer,).is_ok());
    }

    #[test]
    fn node_cannot_connect_to_itself() {
        let local = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let peer = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        assert!(local.validate_peer(&peer,).is_err());
    }

    /*
     * Primeiro teste de rede real:
     *
     * dois sockets TCP locais trocam
     * handshakes completos.
     */
    #[test]
    fn tcp_nodes_exchange_handshakes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server_handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let client_handshake = Handshake::new(node_b_id(), 8, sha256(b"client-tip")).unwrap();

        let expected_client = client_handshake.clone();

        let server_thread =
            thread::spawn(move || accept_one_peer(&listener, &server_handshake).unwrap());

        let received_server = connect_to_peer(&address.to_string(), &client_handshake).unwrap();

        let received_client = server_thread.join().unwrap();

        assert_eq!(received_server.node_id, node_a_id());

        assert_eq!(received_server.height, 10);

        assert_eq!(received_client, expected_client);
    }

    #[test]
    fn tcp_self_connection_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server_handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let client_handshake = Handshake::new(node_a_id(), 10, sample_tip_hash()).unwrap();

        let server_thread = thread::spawn(move || accept_one_peer(&listener, &server_handshake));

        let client_result = connect_to_peer(&address.to_string(), &client_handshake);

        let server_result = server_thread.join().unwrap();

        assert!(server_result.is_err());

        assert!(client_result.is_err());
    }

    #[test]
    fn oversized_handshake_frame_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            configure_stream(&stream).unwrap();

            receive_handshake(&mut stream)
        });

        let mut stream = TcpStream::connect(address).unwrap();

        let oversized = (MAX_HANDSHAKE_FRAME_SIZE + 1) as u32;

        stream.write_all(&oversized.to_le_bytes()).unwrap();

        stream.flush().unwrap();

        let result = server_thread.join().unwrap();

        assert!(result.is_err());
    }
}
