use helium_crypto::PublicKey;
use helium_proto::MapperAttach;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Deserialize, Serialize)]
pub struct ConsoleEvent {
    pub id: String,
    #[serde(with = "serde_payload")]
    pub payload: MapperAttach,
    pub port: u8,
    pub name: PublicKey,
    pub reported_at: usize,
    pub hotspots: Vec<Hotspot>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Hotspot {
    pub lat: f64,
    pub long: f64,
    pub name: String,
}

mod serde_payload {
    use super::*;
    use helium_proto::Message;
    use serde::de;
    use std::fmt;

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<MapperAttach, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PayloadVisitor;

        impl<'de> de::Visitor<'de> for PayloadVisitor {
            type Value = MapperAttach;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("mapper attach payload")
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                let data = base64::decode(value)
                    .map_err(|_| de::Error::custom("invalid payload base64"))?;
                let decoded = MapperAttach::decode(&*data)
                    .map_err(|_| de::Error::custom("invalid payload"))?;
                Ok(decoded)
            }
        }

        deserializer.deserialize_str(PayloadVisitor)
    }

    pub fn serialize<S>(value: &MapperAttach, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let data = value.encode_to_vec();
        serializer.serialize_str(&base64::encode(data))
    }
}
