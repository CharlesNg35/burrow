use std::io::Write;

use super::db::{Db, previous_month, unpack};
use super::db_ip::store;
use super::own_file::OwnFile;
use super::source::{Source, source_for};
use crate::config::settings::DB_IP_FILE;
use crate::policy::geo::Place;

mod fixture {
    use maxminddb_writer::Database;
    use maxminddb_writer::paths::IpAddrWithMask;

    pub fn country_db(entries: &[(&str, &str)], built: u64) -> Vec<u8> {
        let mut db = Database::default();
        db.metadata.database_type = "DBIP-Country-Lite".into();
        db.metadata.binary_format_major_version = 2;
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
        country_db(&[("8.8.8.0/24", "US"), ("203.0.113.0/24", "FR")], built)
    }
}

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

#[test]
fn a_country_database_is_read_and_checked_before_use() {
    let db = Db::from_bytes(fixture::world(1_700_000_000)).unwrap();
    assert_eq!(db.built, 1_700_000_000);
    assert_eq!(
        db.place("203.0.113.4".parse().unwrap()),
        Place::Known("FR".into())
    );
    assert_eq!(
        db.place("::ffff:8.8.8.8".parse().unwrap()),
        Place::Known("US".into())
    );
    assert_eq!(db.place("192.0.2.1".parse().unwrap()), Place::Unknown);
    assert_eq!(db.place("2001:db8::1".parse().unwrap()), Place::Unknown);
    let empty = fixture::country_db(&[("203.0.113.0/24", "FR")], 1);
    assert!(Db::from_bytes(empty).unwrap_err().contains("well-known"));
    assert!(Db::from_bytes(b"not a database".to_vec()).is_err());
    let mut asn = maxminddb_writer::Database::default();
    asn.metadata.database_type = "GeoLite2-ASN".into();
    asn.metadata.binary_format_major_version = 2;
    let r = asn.insert_value(1u32).unwrap();
    asn.insert_node(
        "8.8.8.0/24"
            .parse::<maxminddb_writer::paths::IpAddrWithMask>()
            .unwrap(),
        r,
    );
    let asn = asn.write_to(Vec::new()).unwrap();
    assert!(
        Db::from_bytes(asn)
            .unwrap_err()
            .contains("not a country database")
    );
}

#[test]
fn a_download_is_unpacked_checked_and_kept_only_when_newer() {
    let dir = tempfile::tempdir().unwrap();
    let packed = gz(&fixture::world(2_000));
    let db = store(&packed, dir.path(), 1_000).unwrap();
    assert_eq!(db.built, 2_000);
    let kept = std::fs::read(dir.path().join(DB_IP_FILE)).unwrap();
    assert_eq!(kept, fixture::world(2_000));
    assert!(
        store(&packed, dir.path(), 2_000)
            .unwrap_err()
            .contains("not newer")
    );
    assert!(
        store(b"garbage", dir.path(), 0)
            .unwrap_err()
            .contains("unpack")
    );
    assert!(store(&gz(b"garbage"), dir.path(), 0).is_err());
    assert_eq!(
        std::fs::read(dir.path().join(DB_IP_FILE)).unwrap(),
        kept,
        "a bad download leaves the last good file"
    );
    assert!(unpack(&gz(b"abc")).unwrap() == b"abc");
}

#[test]
fn months_count_back_across_the_year() {
    assert_eq!(previous_month("2026-03"), "2026-02");
    assert_eq!(previous_month("2026-01"), "2025-12");
    assert_eq!(
        super::db::month_url("2026-10"),
        "https://download.db-ip.com/free/dbip-country-lite-2026-10.mmdb.gz"
    );
}

#[tokio::test]
async fn an_own_file_is_read_again_when_it_changes_and_a_broken_one_is_ignored() {
    let geo = super::Geo::default();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("own.mmdb");
    let mut own = OwnFile::new(file.clone());
    geo.reset(Some(&own));
    own.start(&geo);
    assert!(geo.held().is_none());
    assert_eq!(geo.watch.lock().unwrap().result, "file not found");
    std::fs::write(&file, fixture::world(1_000)).unwrap();
    own.tend(&geo).await;
    assert_eq!(geo.held().unwrap().built, 1_000);
    std::fs::write(
        &file,
        b"broken, and longer than before so the stamp changes for sure",
    )
    .unwrap();
    own.tend(&geo).await;
    assert_eq!(
        geo.held().unwrap().built,
        1_000,
        "kept the one already read"
    );
    assert!(geo.watch.lock().unwrap().failed);
    std::fs::write(&file, fixture::world(3_000)).unwrap();
    own.tend(&geo).await;
    assert_eq!(geo.held().unwrap().built, 3_000);
    assert!(!geo.credits(), "an own file is not DB-IP's");
}

#[test]
fn each_source_says_where_its_file_is_and_whom_to_credit() {
    let db_ip = source_for(&crate::config::settings::GeoSource::DbIp {
        dir: Some("/state/geoip".into()),
    });
    assert_eq!(
        db_ip.file().unwrap(),
        std::path::Path::new("/state/geoip").join(DB_IP_FILE)
    );
    assert!(db_ip.credits());
    let own = source_for(&crate::config::settings::GeoSource::File(
        "/etc/own.mmdb".into(),
    ));
    assert_eq!(own.file().unwrap(), std::path::Path::new("/etc/own.mmdb"));
    assert!(!own.credits());
}
