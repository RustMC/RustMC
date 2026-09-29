#!/usr/bin/env python3
"""List 26.3 vanilla registry identifiers in a local-only preview manifest.

The official archive is supplied by the user. No game data is checked in or
copied into RustMC's public tree. This only records identifiers needed for a
locally negotiated `minecraft:core` pack; it does not contain definitions.
"""

import hashlib
import io
import json
import pathlib
import sys
import zipfile

EXPECTED_SHA1 = "33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c"
VERSION = "26.3"
INNER = "META-INF/versions/26.3/server-26.3.jar"

# Version-specific synchronized registry keys from the official 26.3 archive's
# RegistryDataLoader metadata. Keep the manifest local, and review on upgrade.
REGISTRIES = (
    "worldgen/biome",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_sound_variant",
    "cow_variant",
    "chicken_sound_variant",
    "chicken_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "sulfur_cube_archetype",
    "dimension_type",
    "damage_type",
    "banner_pattern",
    "enchantment",
    "jukebox_song",
    "instrument",
    "test_environment",
    "test_instance",
    "dialog",
    "world_clock",
    "timeline",
    "decorated_pot_pattern",
    "block_transformer",
    "worldgen/block_state_provider",
)


def entries(paths: list[str], registry: str) -> list[str]:
    prefix = f"data/minecraft/{registry}/"
    return sorted(
        f"minecraft:{path[len(prefix):-5]}"
        for path in paths
        if path.startswith(prefix) and path.endswith(".json")
    )


def prepare(archive: pathlib.Path, output: pathlib.Path, report: pathlib.Path) -> None:
    if hashlib.sha1(archive.read_bytes()).hexdigest() != EXPECTED_SHA1:
        raise ValueError("official Java 26.3 server archive SHA-1 mismatch")
    static = json.loads(report.read_text())
    with zipfile.ZipFile(archive) as outer:
        version = json.loads(outer.read("version.json"))
        if version["id"] != VERSION or version["protocol_version"] != 777:
            raise ValueError("archive version metadata is not Java 26.3 / 777")
        with zipfile.ZipFile(io.BytesIO(outer.read(INNER))) as inner:
            paths = inner.namelist()
            tags = {}
            all_keys = set(REGISTRIES) | {key.removeprefix("minecraft:") for key in static}
            for registry in sorted(all_keys):
                prefix = f"data/minecraft/tags/{registry}/"
                definitions = {
                    "minecraft:" + path[len(prefix):-5]: json.loads(inner.read(path))["values"]
                    for path in paths if path.startswith(prefix) and path.endswith(".json")
                }
                if registry in REGISTRIES:
                    names = entries(paths, registry)
                else:
                    values = static["minecraft:" + registry]["entries"]
                    names = sorted(values, key=lambda name: values[name]["protocol_id"])
                    if any(values[name]["protocol_id"] != i for i, name in enumerate(names)):
                        raise ValueError("noncontiguous static registry IDs")
                def resolve(tag, trail=()):
                    if tag in trail:
                        raise ValueError("cyclic registry tag")
                    result = set()
                    for value in definitions[tag]:
                        required = True
                        if isinstance(value, dict):
                            required = value.get("required", True)
                            value = value["id"]
                        if value.startswith("#"):
                            if value[1:] in definitions:
                                result.update(resolve(value[1:], trail + (tag,)))
                            elif required:
                                raise ValueError(f"missing tag {value}")
                        elif value in names:
                            result.add(names.index(value))
                        elif required:
                            raise ValueError(f"missing entry {value}")
                    return sorted(result)
                if definitions:
                    tags[registry] = {tag: resolve(tag) for tag in sorted(definitions)}
    lines = [f'version = "{VERSION}"', "[registries]"]
    total = 0
    for registry in REGISTRIES:
        names = entries(paths, registry)
        if not names and registry not in (
            "dialog",
            "test_environment",
            "test_instance",
            "worldgen/block_state_provider",
        ):
            raise ValueError(f"expected entries for {registry}")
        total += len(names)
        lines.append(f'{json.dumps("minecraft:" + registry)} = [')
        lines.extend(f"  {json.dumps(name)}," for name in names)
        lines.append("]")
    lines.append("[static_registry_sizes]")
    for registry in tags:
        if registry not in REGISTRIES:
            key = "minecraft:" + registry
            lines.append(f"{json.dumps(key)} = {len(static[key]['entries'])}")
    for registry, values in tags.items():
        lines.append(f'[tags.{json.dumps("minecraft:" + registry)}]')
        for tag, ids in values.items():
            lines.append(f"{json.dumps(tag)} = {json.dumps(ids)}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(lines) + "\n")
    print(f"Wrote {len(REGISTRIES)} registry lists, {total} identifiers to {output}")
    print("Keep this generated manifest local; do not commit or redistribute it.")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("usage: prepare_preview_registry.py OFFICIAL_SERVER_JAR LOCAL_OUTPUT_TOML OFFICIAL_REGISTRIES_REPORT")
    prepare(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]))
