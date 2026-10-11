//! Read only comparison-critical metadata from a local Java 26.3 save.
//! Usage: inspect_vanilla_save WORLD_DIRECTORY
//!
//! The parsing itself lives in `rustmc_tools::save`, the single metadata
//! reader shared with `vanilla_oracle`, so both tools always agree on the
//! stored seed and version.

use std::path::Path;

use rustmc_tools::save;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        eprintln!("usage: inspect_vanilla_save WORLD_DIRECTORY");
        std::process::exit(2);
    }
    match save::inspect(Path::new(&args[0])) {
        Ok(description) => println!("{description}"),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use rustmc_tools::nbt::Tag;
    use std::collections::BTreeMap;

    fn compound(entries: impl IntoIterator<Item = (&'static str, Tag)>) -> Tag {
        Tag::Compound(
            entries
                .into_iter()
                .map(|(name, tag)| (name.to_owned(), tag))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn write_compound(entries: &BTreeMap<String, Tag>, out: &mut Vec<u8>) {
        for (name, tag) in entries {
            out.push(match tag {
                Tag::Compound(_) => 10,
                Tag::Long(_) => 4,
                Tag::String(_) => 8,
                _ => panic!("test fixture uses only compound, long, and string"),
            });
            out.extend((name.len() as u16).to_be_bytes());
            out.extend(name.as_bytes());
            match tag {
                Tag::Compound(inner) => write_compound(inner, out),
                Tag::Long(value) => out.extend(value.to_be_bytes()),
                Tag::String(value) => {
                    out.extend((value.len() as u16).to_be_bytes());
                    out.extend(value.as_bytes());
                }
                _ => unreachable!(),
            }
        }
        out.push(0);
    }

    /// The bin's stdout is the shared reader's description, so the fixture
    /// that used to be parsed here is now encoded and handed to
    /// `rustmc_tools::save::describe`.
    #[test]
    fn reads_exact_seed_and_generator_without_personal_fields() {
        let level = compound([(
            "Data",
            compound([
                ("Version", compound([("Name", Tag::String("26.3".into()))])),
                ("Player", Tag::String("private".into())),
            ]),
        )]);
        let settings = compound([(
            "data",
            compound([
                ("seed", Tag::Long(-2026)),
                (
                    "dimensions",
                    compound([(
                        "minecraft:overworld",
                        compound([(
                            "generator",
                            compound([
                                ("type", Tag::String("minecraft:noise".into())),
                                ("settings", Tag::String("minecraft:overworld".into())),
                            ]),
                        )]),
                    )]),
                ),
            ]),
        )]);
        let encode = |tag| {
            let Tag::Compound(root) = tag else {
                unreachable!()
            };
            let mut bytes = vec![10, 0, 0];
            write_compound(&root, &mut bytes);
            bytes
        };
        let output = rustmc_tools::save::describe(&encode(level), &encode(settings)).unwrap();
        assert!(output.contains("seed=-2026"));
        assert!(output.contains("overworld_settings=minecraft:overworld"));
        assert!(!output.contains("private"));
    }
}
