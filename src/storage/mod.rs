use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    block::{Block, MAX_BLOCK_SIZE},
    blockchain::Blockchain,
};

const STORAGE_MAGIC: &[u8; 8] = b"TPZCHAIN";

const STORAGE_VERSION: u16 = 2;
const MAX_STORAGE_BYTES: usize = 256 * 1024 * 1024;
const MAX_BLOCK_COUNT: usize = 1_000_000;

pub fn save_blockchain(blockchain: &Blockchain, path: impl AsRef<Path>) -> Result<(), String> {
    blockchain.validate()?;

    let path = path.as_ref();

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create storage directory: {}", error))?;
        }
    }

    let bytes = encode_blockchain(blockchain)?;

    /*
     * Antes de tocar no arquivo principal,
     * verificamos se os bytes que acabamos
     * de gerar realmente conseguem
     * reconstruir uma blockchain válida.
     */
    decode_blockchain(&bytes)
        .map_err(|error| format!("refusing to save invalid serialized blockchain: {}", error))?;

    let temporary_path = temporary_storage_path(path)?;

    write_and_sync_file(&temporary_path, &bytes)?;

    /*
     * Conferimos também o arquivo físico
     * temporário antes da substituição.
     */
    load_blockchain(&temporary_path).map_err(|error| {
        let _ = fs::remove_file(&temporary_path);

        format!("temporary blockchain verification failed: {}", error)
    })?;

    if let Err(error) = replace_storage_file(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);

        return Err(error);
    }

    Ok(())
}

pub fn load_blockchain(path: impl AsRef<Path>) -> Result<Blockchain, String> {
    let bytes = fs::read(path.as_ref())
        .map_err(|error| format!("failed to read blockchain storage: {}", error))?;

    if bytes.len() > MAX_STORAGE_BYTES {
        return Err("blockchain storage exceeds the network limit".to_string());
    }

    decode_blockchain(&bytes)
}

fn encode_blockchain(blockchain: &Blockchain) -> Result<Vec<u8>, String> {
    let block_count =
        u64::try_from(blockchain.len()).map_err(|_| "block count is too large".to_string())?;

    let mut bytes = Vec::new();

    bytes.extend_from_slice(STORAGE_MAGIC);

    bytes.extend_from_slice(&STORAGE_VERSION.to_le_bytes());

    bytes.extend_from_slice(&blockchain.difficulty_bits().to_le_bytes());

    bytes.extend_from_slice(&block_count.to_le_bytes());

    for block in blockchain.blocks() {
        let encoded = block.encode();

        let block_length =
            u64::try_from(encoded.len()).map_err(|_| "encoded block is too large".to_string())?;

        bytes.extend_from_slice(&block_length.to_le_bytes());

        bytes.extend_from_slice(&encoded);
    }

    Ok(bytes)
}

fn decode_blockchain(bytes: &[u8]) -> Result<Blockchain, String> {
    let mut cursor = 0usize;

    let magic = read_bytes(bytes, &mut cursor, STORAGE_MAGIC.len())?;

    if magic != STORAGE_MAGIC {
        return Err("invalid Topazio blockchain storage magic".to_string());
    }

    let version = read_u16(bytes, &mut cursor)?;

    if version != STORAGE_VERSION {
        return Err(format!("unsupported storage version: {}", version));
    }

    let difficulty_bits = read_u16(bytes, &mut cursor)?;

    let block_count = usize::try_from(read_u64(bytes, &mut cursor)?)
        .map_err(|_| "stored block count is too large".to_string())?;

    if block_count == 0 {
        return Err("stored blockchain contains zero blocks".to_string());
    }

    if block_count > MAX_BLOCK_COUNT {
        return Err("stored blockchain contains too many blocks".to_string());
    }

    let mut blocks = Vec::with_capacity(block_count);

    for _ in 0..block_count {
        let block_length = usize::try_from(read_u64(bytes, &mut cursor)?)
            .map_err(|_| "stored block length is too large".to_string())?;

        if block_length > MAX_BLOCK_SIZE {
            return Err("stored block exceeds the network limit".to_string());
        }

        let block_bytes = read_bytes(bytes, &mut cursor, block_length)?;

        let block = Block::decode(block_bytes)?;

        blocks.push(block);
    }

    if cursor != bytes.len() {
        return Err("blockchain storage contains trailing bytes".to_string());
    }

    Blockchain::from_blocks(difficulty_bits, blocks)
}

fn write_and_sync_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("failed to create temporary blockchain storage: {}", error))?;

    file.write_all(bytes)
        .map_err(|error| format!("failed to write temporary blockchain storage: {}", error))?;

    file.flush()
        .map_err(|error| format!("failed to flush temporary blockchain storage: {}", error))?;

    file.sync_all().map_err(|error| {
        format!(
            "failed to synchronize temporary blockchain storage: {}",
            error
        )
    })?;

    drop(file);

    Ok(())
}

fn replace_storage_file(temporary_path: &Path, destination_path: &Path) -> Result<(), String> {
    /*
     * Se ainda não existe blockchain
     * anterior, basta promover o arquivo
     * temporário.
     */
    if !destination_path.exists() {
        fs::rename(temporary_path, destination_path).map_err(|error| {
            format!("failed to promote temporary blockchain storage: {}", error)
        })?;

        return Ok(());
    }

    let backup_path = backup_storage_path(destination_path);

    if backup_path.exists() {
        fs::remove_file(&backup_path)
            .map_err(|error| format!("failed to remove stale blockchain backup: {}", error))?;
    }

    /*
     * Primeiro preservamos o arquivo
     * atualmente válido.
     */
    fs::rename(destination_path, &backup_path)
        .map_err(|error| format!("failed to preserve previous blockchain storage: {}", error))?;

    /*
     * Depois promovemos o novo arquivo.
     */
    if let Err(error) = fs::rename(temporary_path, destination_path) {
        /*
         * Se a promoção falhar,
         * tentamos restaurar imediatamente
         * o arquivo anterior.
         */
        let restore_result = fs::rename(&backup_path, destination_path);

        return match restore_result {
            Ok(_) => Err(format!(
                "failed to replace blockchain storage; previous storage was restored: {}",
                error
            )),

            Err(restore_error) => Err(format!(
                "failed to replace blockchain storage and failed to restore backup: replace error: {}; restore error: {}",
                error, restore_error
            )),
        };
    }

    /*
     * O novo arquivo já está no lugar.
     * O backup pode ser removido.
     */
    if let Err(error) = fs::remove_file(&backup_path) {
        /*
         * Isso não invalida a blockchain
         * nova. Apenas deixou um backup
         * antigo no disco.
         */
        eprintln!(
            "warning: failed to remove blockchain backup {}: {}",
            backup_path.display(),
            error
        );
    }

    Ok(())
}

fn temporary_storage_path(destination_path: &Path) -> Result<PathBuf, String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            format!(
                "system clock error while creating temporary storage name: {}",
                error
            )
        })?
        .as_nanos();

    let file_name = destination_path
        .file_name()
        .ok_or_else(|| "blockchain storage path has no file name".to_string())?
        .to_string_lossy();

    let temporary_name = format!("{}.tmp-{}-{}", file_name, std::process::id(), unique);

    Ok(destination_path.with_file_name(temporary_name))
}

fn backup_storage_path(destination_path: &Path) -> PathBuf {
    let file_name = destination_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "topazio-chain".to_string());

    destination_path.with_file_name(format!("{}.bak", file_name))
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, String> {
    let raw = read_bytes(bytes, cursor, 2)?;

    let mut array = [0u8; 2];

    array.copy_from_slice(raw);

    Ok(u16::from_le_bytes(array))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let raw = read_bytes(bytes, cursor, 8)?;

    let mut array = [0u8; 8];

    array.copy_from_slice(raw);

    Ok(u64::from_le_bytes(array))
}

fn read_bytes<'a>(bytes: &'a [u8], cursor: &mut usize, length: usize) -> Result<&'a [u8], String> {
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| "storage decode offset overflow".to_string())?;

    if end > bytes.len() {
        return Err("unexpected end of blockchain storage".to_string());
    }

    let result = &bytes[*cursor..end];

    *cursor = end;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        consensus::INITIAL_BLOCK_REWARD, identity::KeyPair, mining::mine_block,
        transaction::Transaction,
    };

    fn temporary_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "topazio-{}-{}-{}.dat",
            name,
            std::process::id(),
            unique
        ))
    }

    fn mine_reward_block(blockchain: &Blockchain, miner: &KeyPair) -> Block {
        let coinbase = Transaction::coinbase(
            miner.address(),
            INITIAL_BLOCK_REWARD,
            blockchain.next_height(),
        )
        .unwrap();

        let candidate = Block::with_transactions_at_difficulty(
            blockchain.next_height(),
            blockchain.tip().hash(),
            1_788_825_600 + blockchain.next_height(),
            0,
            blockchain.difficulty_bits(),
            &[coinbase],
        )
        .unwrap();

        mine_block(candidate, blockchain.difficulty_bits(), 1_000_000).unwrap()
    }

    #[test]
    fn blockchain_can_be_saved_and_loaded() {
        let path = temporary_path("save-load");

        let miner = KeyPair::from_secret_bytes([7u8; 32]);

        let mut blockchain = Blockchain::new();

        for _ in 0..2 {
            let block = mine_reward_block(&blockchain, &miner);

            blockchain.append_block(block).unwrap();
        }

        save_blockchain(&blockchain, &path).unwrap();

        let loaded = load_blockchain(&path).unwrap();

        assert_eq!(blockchain.blocks(), loaded.blocks());

        assert_eq!(blockchain.state(), loaded.state());

        assert_eq!(
            blockchain.total_supply().unwrap(),
            loaded.total_supply().unwrap()
        );

        assert!(loaded.validate().is_ok());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_reload_preserves_immature_coinbase_balances() {
        let path = temporary_path("maturity-reload");
        let miner = KeyPair::from_secret_bytes([17u8; 32]);
        let mut blockchain = Blockchain::with_difficulty(0);
        blockchain
            .append_block(mine_reward_block(&blockchain, &miner))
            .unwrap();

        save_blockchain(&blockchain, &path).unwrap();
        let loaded = load_blockchain(&path).unwrap();

        assert_eq!(loaded.state(), blockchain.state());
        assert_eq!(loaded.state().current_height(), 1);
        assert_eq!(loaded.state().spendable_balance_of(&miner.address()), 0);
        assert_eq!(
            loaded.state().immature_balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn existing_storage_can_be_safely_replaced() {
        let path = temporary_path("replace");

        let miner = KeyPair::from_secret_bytes([7u8; 32]);

        let mut blockchain = Blockchain::new();

        let block_1 = mine_reward_block(&blockchain, &miner);

        blockchain.append_block(block_1).unwrap();

        save_blockchain(&blockchain, &path).unwrap();

        let first_loaded = load_blockchain(&path).unwrap();

        assert_eq!(first_loaded.tip().height, 1);

        let block_2 = mine_reward_block(&blockchain, &miner);

        blockchain.append_block(block_2).unwrap();

        save_blockchain(&blockchain, &path).unwrap();

        let second_loaded = load_blockchain(&path).unwrap();

        assert_eq!(second_loaded.tip().height, 2);

        assert_eq!(
            second_loaded.total_supply().unwrap(),
            INITIAL_BLOCK_REWARD * 2
        );

        assert!(!backup_storage_path(&path,).exists());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_runtime_state_is_not_saved() {
        let path = temporary_path("invalid-state");

        let miner = KeyPair::from_secret_bytes([7u8; 32]);

        let mut blockchain = Blockchain::new();

        let block = mine_reward_block(&blockchain, &miner);

        blockchain.append_block(block).unwrap();

        save_blockchain(&blockchain, &path).unwrap();

        blockchain
            .state_mut_for_lab()
            .set_balance_for_lab(miner.address(), 999_999);

        assert!(save_blockchain(&blockchain, &path,).is_err());

        let loaded = load_blockchain(&path).unwrap();

        assert_eq!(
            loaded.state().balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD
        );

        assert_eq!(loaded.tip().height, 1);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_magic_is_rejected() {
        let path = temporary_path("bad-magic");

        fs::write(&path, b"NOTTOPAZIO").unwrap();

        assert!(load_blockchain(&path).is_err());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn truncated_storage_is_rejected() {
        let path = temporary_path("truncated");

        let mut bytes = Vec::new();

        bytes.extend_from_slice(STORAGE_MAGIC);

        bytes.extend_from_slice(&STORAGE_VERSION.to_le_bytes());

        fs::write(&path, bytes).unwrap();

        assert!(load_blockchain(&path).is_err());

        let _ = fs::remove_file(path);
    }
}
