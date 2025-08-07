use bincode::config::Configuration;

pub(crate) fn serialization_config() -> Configuration {
    bincode::config::standard()
}
