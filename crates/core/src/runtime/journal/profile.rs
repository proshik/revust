use super::Journal;
use crate::plan::ProfileSnapshot;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;

#[derive(Serialize)]
struct Profile<'a> {
    version: u32,
    profile: &'a str,
    source: &'a std::path::Path,
    sha256: String,
    content: &'a str,
}

impl Journal {
    /// Keep the complete selected extension profile outside the legacy prompt tree.
    /// Use the frozen plan contents; never reopen the original source file.
    pub fn save_extension_profile(
        &self,
        profile: &str,
        snapshot: &ProfileSnapshot,
    ) -> Result<(), String> {
        let metadata = Profile {
            version: 1,
            profile,
            source: &snapshot.source,
            sha256: Sha256::digest(snapshot.content.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            content: &snapshot.content,
        };
        self.save_preparation_metadata("extension-profile.json", &metadata)
    }

    /// Archive resolved policy and origins without exposing child environment values.
    pub fn save_policy(
        &self,
        profile: &str,
        policy: &crate::config::extensions::EffectivePolicy,
    ) -> Result<(), String> {
        self.save_preparation_metadata(
            "policy.json",
            &serde_json::json!({"version":1,"profile":profile,"policy":policy.inspection()}),
        )
    }

    fn save_preparation_metadata(
        &self,
        name: &str,
        metadata: &impl Serialize,
    ) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "journal lock poisoned")?;
        state.writable()?;
        let result = (|| {
            if !state.attempts.is_empty() {
                return Err("preparation metadata must precede dispatch".to_owned());
            }
            let mut bytes = serde_json::to_vec(metadata).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            let mut file = super::private_file(&self.directory, name).map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .and_then(|_| self.directory.sync_all())
                .map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            state.fail();
            return Err(format!("persist {name}: {error}"));
        }
        Ok(())
    }
}
