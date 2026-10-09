use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{
    Deserializer, Serializer,
    de::{Error, SeqAccess, Visitor},
};

pub(super) fn serialize<S: Serializer>(content: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&STANDARD.encode(content))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    deserializer.deserialize_any(FileContent)
}

struct FileContent;

impl<'de> Visitor<'de> for FileContent {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("base64 file content or a legacy byte array")
    }

    fn visit_str<E: Error>(self, content: &str) -> Result<Self::Value, E> {
        STANDARD.decode(content).map_err(E::custom)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut content = Vec::new();
        while let Some(byte) = sequence.next_element::<u8>()? {
            content.push(byte);
        }
        Ok(content)
    }
}
