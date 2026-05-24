use crate::server::BusServerError;
use dsl_bus_protocol::v1::Uuid as ProtoUuid;

pub(crate) fn to_proto(u: uuid::Uuid) -> ProtoUuid {
    ProtoUuid {
        value: u.as_bytes().to_vec(),
    }
}

pub(crate) fn from_proto(p: &ProtoUuid) -> Result<uuid::Uuid, BusServerError> {
    if p.value.len() != 16 {
        return Err(BusServerError::MalformedUuid {
            actual_len: p.value.len(),
        });
    }
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&p.value);
    Ok(uuid::Uuid::from_bytes(bytes))
}

pub(crate) fn from_proto_opt(opt: &Option<ProtoUuid>) -> Result<Option<uuid::Uuid>, BusServerError> {
    match opt {
        Some(p) => from_proto(p).map(Some),
        None => Ok(None),
    }
}
