use super::*;

const HASH: &str = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";

fn nixpkgs(url: &str) -> Entry {
    Entry::Toolchain(ToolchainPin {
        name: ToolchainSource::Nixpkgs,
        url: Url::new(url).unwrap(),
        nar_hash: NarHash::parse(HASH).unwrap(),
    })
}

const LINE: &str = r#"{"ecosystem":"toolchain","name":"nixpkgs","url":"https://example.invalid/nixpkgs.tar.gz","narHash":"sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="}"#;

#[test]
fn tracked_schemas_match_the_types() {
    for (tracked, generated, file) in [
        (
            include_str!("../schema/lock.schema.json"),
            render_json(&header_schema()),
            "lock.schema.json",
        ),
        (
            include_str!("../schema/lock-entry.schema.json"),
            render_json(&entry_schema()),
            "lock-entry.schema.json",
        ),
    ] {
        assert!(
            tracked == generated,
            "crates/lock/schema/{file} is stale; it has to be:\n{generated}"
        );
    }
}

#[test]
fn an_entry_is_one_line_tagged_with_its_ecosystem() {
    let entry = nixpkgs("https://example.invalid/nixpkgs.tar.gz");
    assert_eq!(entry.line(), LINE);
    assert_eq!(Entry::parse(LINE).unwrap(), entry);
    assert_eq!(entry.shard(), "shikumi.lock/toolchain/nixpkgs.jsonl");
}

#[test]
fn a_line_written_any_other_way_is_refused() {
    let respaced = LINE.replace(",", ", ");
    let reordered = r#"{"name":"nixpkgs","ecosystem":"toolchain","url":"https://example.invalid/nixpkgs.tar.gz","narHash":"sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="}"#;
    for line in [respaced.as_str(), reordered] {
        let error = Entry::parse(line).unwrap_err();
        assert!(
            error.contains("isn't written as Shikumi writes it"),
            "{error}"
        );
        assert!(error.ends_with(LINE), "{error}");
    }
}

#[test]
fn an_entry_outside_the_contract_is_refused() {
    for (line, says) in [
        (
            LINE.replace(r#""narHash""#, r#""extra":1,"narHash""#),
            "unknown field `extra`",
        ),
        (
            LINE.replace("toolchain", "crates"),
            "unknown variant `crates`",
        ),
        (
            LINE.replace(r#""nixpkgs""#, r#""nixos""#),
            "unknown variant `nixos`",
        ),
        (
            LINE.replace(HASH, "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73"),
            "isn't a sha256 written as `sha256-<base64>`",
        ),
        (
            LINE.replace("https://", "ftp://"),
            "isn't a URL Shikumi fetches",
        ),
        (
            LINE.replace(r#""ecosystem":"toolchain","#, ""),
            "missing field `ecosystem`",
        ),
    ] {
        let error = Entry::parse(&line).unwrap_err();
        assert!(error.contains(says), "{line}: {error}");
    }
}

// the base-64 digit before the padding carries four bits of the hash and two that have to be zero.
#[test]
fn a_hash_with_bits_past_its_end_is_refused() {
    assert!(NarHash::parse(&HASH.replace("FU=", "FV=")).is_err());
    assert!(NarHash::parse(&HASH.replace("FU=", "FU")).is_err());
    assert!(NarHash::parse(HASH).is_ok());
}

// each entry's tag is its ecosystem's name, and every ecosystem has an entry shape.
#[test]
fn every_ecosystem_tags_its_entries_with_its_name() {
    for ecosystem in Ecosystem::ALL {
        let entry = match ecosystem {
            Ecosystem::Toolchain => nixpkgs("https://example.invalid/nixpkgs.tar.gz"),
        };
        assert_eq!(entry.ecosystem(), *ecosystem);
        let tagged: serde_json::Value = serde_json::from_str(&entry.line()).unwrap();
        assert_eq!(tagged["ecosystem"], ecosystem.as_str());
    }
}

// a shard's lines end in `\n` alone, so a line Windows wrote is one Shikumi didn't.
#[test]
fn a_shard_with_crlf_endings_is_refused() {
    let header = Header::text();
    let crlf = format!("{LINE}\r\n");
    let root = repository(&[
        ("shikumi.lock/lock.json", &header),
        ("shikumi.lock/toolchain/nixpkgs.jsonl", &crlf),
    ]);
    let error = read_all(
        root.path(),
        &[
            "shikumi.lock/lock.json",
            "shikumi.lock/toolchain/nixpkgs.jsonl",
        ],
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("isn't written as Shikumi writes it"),
        "{error}"
    );
}

#[test]
fn every_shard_names_its_ecosystem_and_package() {
    for &ecosystem in Ecosystem::ALL {
        for package in ["nixpkgs", "a.b-c_d"] {
            let path = shard(ecosystem, package);
            assert_eq!(shard_of(&path), Some((ecosystem, package)), "{path}");
        }
    }
    for path in [
        "shikumi.lock/lock.json",
        "shikumi.lock/toolchain.jsonl",
        "shikumi.lock/crates/serde.jsonl",
        "shikumi.lock/toolchain/nixpkgs.json",
        "shikumi.lock/toolchain/.jsonl",
        "shikumi.lock/toolchain/a/b.jsonl",
        "elsewhere/toolchain/nixpkgs.jsonl",
    ] {
        assert_eq!(shard_of(path), None, "{path}");
    }
}

/// a repository whose lock holds the given files.
fn repository(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (file, text) in files {
        let path = root.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

fn read_all(root: &Path, files: &[&str]) -> Result<Lock, LockError> {
    Lock::read(root, files.iter().copied())
}

fn shard_text(lines: &[&str]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

#[test]
fn a_lock_reads_as_its_tracked_shards() {
    let header = Header::text();
    let shard = shard_text(&[LINE]);
    let root = repository(&[
        ("shikumi.lock/lock.json", &header),
        ("shikumi.lock/toolchain/nixpkgs.jsonl", &shard),
        ("shikumi.lock/toolchain/nix-unit.jsonl", "not even JSON\n"),
    ]);
    // the broken shard isn't tracked, so it isn't part of the lock.
    let lock = read_all(
        root.path(),
        &[
            "shikumi.lock/toolchain/nixpkgs.jsonl",
            "shikumi.lock/lock.json",
        ],
    )
    .unwrap();
    assert_eq!(
        lock.entries(),
        [nixpkgs("https://example.invalid/nixpkgs.tar.gz")]
    );
    assert!(lock.required(ToolchainSource::Nixpkgs).is_ok());
    let unpinned = lock.required(ToolchainSource::NixUnit).unwrap_err();
    assert_eq!(
        unpinned.to_string(),
        "the lock pins no nix-unit; pin it with `shikumi lock pin nix-unit <url>`"
    );
}

#[test]
fn a_broken_lock_is_refused_naming_where() {
    let header = Header::text();
    let one = shard_text(&[LINE]);
    let twice = shard_text(&[LINE, LINE]);
    let moved = LINE.replace(r#""nixpkgs""#, r#""nix-unit""#);
    let misplaced = shard_text(&[&moved]);
    let old = header.replace(r#""minor": 0"#, r#""minor": 1"#);
    let cases: [(&[(&str, &str)], &str); 7] = [
        (
            &[("shikumi.lock/toolchain/nixpkgs.jsonl", &one)],
            "has no lock: Shikumi reads every pin from shikumi.lock/lock.json",
        ),
        (
            &[
                ("shikumi.lock/lock.json", &header),
                ("shikumi.lock/notes.txt", "hi\n"),
            ],
            "shikumi.lock/notes.txt is in the lock but isn't shikumi.lock/lock.json or a shard",
        ),
        (
            &[("shikumi.lock/lock.json", &old)],
            "shikumi.lock/lock.json: version 1.1 is not the supported version 1.0",
        ),
        (
            &[
                ("shikumi.lock/lock.json", &header),
                ("shikumi.lock/toolchain/nixpkgs.jsonl", &twice),
            ],
            "shikumi.lock/toolchain/nixpkgs.jsonl:2: the shard's lines are sorted, each once",
        ),
        (
            &[
                ("shikumi.lock/lock.json", &header),
                ("shikumi.lock/toolchain/nixpkgs.jsonl", &misplaced),
            ],
            "shikumi.lock/toolchain/nixpkgs.jsonl:1: the entry belongs in shikumi.lock/toolchain/nix-unit.jsonl",
        ),
        (
            &[
                ("shikumi.lock/lock.json", &header),
                ("shikumi.lock/toolchain/nixpkgs.jsonl", ""),
            ],
            "shikumi.lock/toolchain/nixpkgs.jsonl:1: a shard holds at least one entry",
        ),
        (
            &[
                ("shikumi.lock/lock.json", &header),
                ("shikumi.lock/toolchain/nixpkgs.jsonl", LINE),
            ],
            "on a line of its own",
        ),
    ];
    for (files, says) in cases {
        let root = repository(files);
        let names: Vec<&str> = files.iter().map(|(file, _)| *file).collect();
        let error = read_all(root.path(), &names).unwrap_err().to_string();
        assert!(error.contains(says), "{names:?}: {error}");
    }
}

#[test]
fn pinning_writes_the_header_and_replaces_the_sources_entry() {
    let root = tempfile::tempdir().unwrap();
    let written = pin(root.path(), &nixpkgs("https://example.invalid/old.tar.gz")).unwrap();
    assert_eq!(
        written,
        [
            "shikumi.lock/lock.json",
            "shikumi.lock/toolchain/nixpkgs.jsonl"
        ]
    );
    let moved = nixpkgs("https://example.invalid/nixpkgs.tar.gz");
    assert_eq!(
        pin(root.path(), &moved).unwrap(),
        ["shikumi.lock/toolchain/nixpkgs.jsonl"]
    );
    let shard = std::fs::read_to_string(root.path().join(moved.shard())).unwrap();
    assert_eq!(shard, format!("{LINE}\n"));
    let lock = read_all(root.path(), &[&header(), &moved.shard()]).unwrap();
    assert_eq!(lock.entries(), [moved]);
    let leftovers: Vec<_> = std::fs::read_dir(root.path().join("shikumi.lock/toolchain"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["nixpkgs.jsonl"]);
}

// the flake's nixpkgs input as the checks pin it: its lock's `narHash` and the path every
// implementation registers for it.
#[test]
fn a_toolchain_pin_names_the_path_its_tree_has() {
    let pin = ToolchainPin {
        name: ToolchainSource::Nixpkgs,
        url: Url::new("https://example.invalid/nixpkgs.tar.gz").unwrap(),
        nar_hash: NarHash::parse(HASH).unwrap(),
    };
    assert_eq!(
        *pin.nar_hash.sha256(),
        shikumi_derivation::Sha256::digest(b""),
    );
    let store = StoreDir::new("/nix/store").unwrap();
    // `nix-store --print-fixed-path --recursive sha256 <hash> source` on Lix 2.95.4.
    assert_eq!(
        store.display(&pin.store_path(&store)),
        store.display(&store.fixed_output_path(
            "source",
            &FixedHash {
                ingestion: Ingestion::Recursive,
                sha256: shikumi_derivation::Sha256::digest(b"")
            }
        ))
    );
}
