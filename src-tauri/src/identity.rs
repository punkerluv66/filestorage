use anyhow::{Context, Result, anyhow};
use iroh::SecretKey;
use keyring::v1::{Entry, Error};
use zeroize::Zeroizing;

const SERVICE: &str = "local.filestorage.desktop";
const ACCOUNT: &str = "iroh-device-key";

pub fn load_or_create() -> Result<SecretKey> {
    let entry = Entry::new(SERVICE, ACCOUNT).map_err(|_| {
        anyhow!(
            "The system credential store is unavailable: {:?}",
            Entry::store_status().as_ref().err()
        )
    })?;

    match entry.get_secret() {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            let key: &[u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("The saved device key is invalid"))?;
            Ok(SecretKey::from_bytes(key))
        }
        Err(Error::NoEntry) => {
            let key = SecretKey::generate();
            let bytes = Zeroizing::new(key.to_bytes());
            entry
                .set_secret(bytes.as_ref())
                .context("Could not save the device key to the system credential store")?;
            Ok(key)
        }
        Err(error) => {
            Err(error).context("Could not read the device key from the system credential store")
        }
    }
}
