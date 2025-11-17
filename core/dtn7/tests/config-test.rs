use config::{Config, File};

#[test]
fn config_test() {
    // Start off by merging in the "default" configuration file
    let s = Config::builder()
        .add_source(Config::default())
        .add_source(File::new(
            "../../examples/dtn7.toml.example",
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap();

    println!("{:?}", s);

    println!("debug: {:?}", s.get_bool("debug").unwrap_or(false));
    println!("nodeid: {:?}", s.get_string("nodeid").unwrap());
    println!("routing: {:?}", s.get_string("routing.strategy").unwrap());
    println!("janitor: {:?}", s.get_string("core.janitor").unwrap());
    println!("workdir: {:?}", s.get_string("workdir").unwrap());
    println!("db: {:?}", s.get_string("db").unwrap());

    println!(
        "discovery-interval: {:?}",
        s.get_string("discovery.interval").unwrap()
    );
    println!(
        "discovery-peer-timeout: {:?}",
        s.get_string("discovery.peer-timeout").unwrap()
    );

    let peers = s.get_array("statics.peers");

    for m in peers.unwrap().iter() {
        println!("Peer: {:?}", m.clone().into_string().unwrap());
    }

    let endpoints = s.get_table("endpoints.local");

    for (_k, v) in endpoints.unwrap().iter() {
        println!("EID: {:?}", v.clone().into_string().unwrap());
    }

    let clas = s.get_table("convergencylayers.cla");
    for (_k, v) in clas.unwrap().iter() {
        let tab = v.clone().into_table().unwrap();
        println!("CLA: {:?}", tab["id"].clone().into_string().unwrap());
    }
}

#[test]
fn config_optional_agents() {
    use dtn7::DtnConfig;
    use std::path::PathBuf;

    // Test config with both agents enabled (from example)
    let cfg = DtnConfig::from(PathBuf::from("../../examples/dtn7.toml.example"));
    assert!(
        cfg.webport.is_some(),
        "webport should be Some when specified in config"
    );
    assert!(
        cfg.unix_socket_path.is_some(),
        "unix_socket_path should be Some when specified in config"
    );

    // Test default config has both agents enabled
    let cfg_default = DtnConfig::new();
    assert!(
        cfg_default.webport.is_some(),
        "default config should have webport enabled"
    );
    assert!(
        cfg_default.unix_socket_path.is_some(),
        "default config should have unix_socket_path enabled"
    );

    // Test config with agents omitted
    let cfg_no_agents = DtnConfig::from(PathBuf::from("tests/test_no_agents.toml"));
    assert!(
        cfg_no_agents.webport.is_none(),
        "webport should be None when omitted from config"
    );
    assert!(
        cfg_no_agents.unix_socket_path.is_none(),
        "unix_socket_path should be None when omitted from config"
    );
}
