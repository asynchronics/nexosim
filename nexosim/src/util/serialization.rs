use bincode::config::{LittleEndian, Varint};

pub(crate) fn get_serialization_config() -> bincode::config::Configuration<LittleEndian, Varint> {
    bincode::config::standard()
}
