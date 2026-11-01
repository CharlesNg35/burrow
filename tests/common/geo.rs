use maxminddb_writer::Database;
use maxminddb_writer::paths::IpAddrWithMask;

pub fn country_db(entries: &[(&str, &str)], built: u64) -> Vec<u8> {
    let mut db = Database::default();
    db.metadata.database_type = "DBIP-Country-Lite".into();
    db.metadata.binary_format_major_version = 2;
    db.metadata.languages = vec!["en".into()];
    db.metadata.build_epoch = built;
    for (net, code) in entries {
        let record = db
            .insert_value(serde_json::json!({ "country": { "iso_code": code } }))
            .unwrap();
        db.insert_node(net.parse::<IpAddrWithMask>().unwrap(), record);
    }
    db.write_to(Vec::new()).unwrap()
}

pub fn world(built: u64) -> Vec<u8> {
    country_db(
        &[
            ("8.8.8.0/24", "US"),
            ("1.1.1.0/24", "AU"),
            ("203.0.113.0/24", "FR"),
            ("198.51.100.0/24", "US"),
        ],
        built,
    )
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
