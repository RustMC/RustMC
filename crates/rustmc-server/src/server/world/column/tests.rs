//! Unit tests for `server::world::column`.

use super::*;

#[test]
fn uniform_sections_promote_on_first_different_write() {
    let mut column = Column::filled(2, -3, 0, 1);
    assert_eq!(column.memory_bytes(), CHUNK_BIOMES * 4);
    assert_eq!(column.set(1, 64, 2, 0), Some(0), "same state stays uniform");
    assert_eq!(column.memory_bytes(), CHUNK_BIOMES * 4);
    assert_eq!(column.set(1, 64, 2, 7), Some(0));
    assert_eq!(column.get(1, 64, 2), Some(7));
    assert_eq!(column.get(1, 65, 2), Some(0));
    assert_eq!(column.memory_bytes(), SECTION_CELLS * 4 + CHUNK_BIOMES * 4);
}

#[test]
fn out_of_range_positions_are_refused() {
    let mut column = Column::filled(0, 0, 0, 0);
    assert_eq!(column.get(0, OVERWORLD_MIN_Y - 1, 0), None);
    assert_eq!(column.get(0, MAX_Y, 0), None);
    assert_eq!(column.set(16, 0, 0, 1), None);
    assert_eq!(column.get(0, OVERWORLD_MIN_Y, 0), Some(0));
}

#[test]
fn adapter_view_matches_reads() {
    let mut column = Column::filled(5, 6, 1, 3);
    column.fill_section(0, 9);
    column.set(15, 100, 15, 4);
    column.set_biome_column(1, 2, 8);
    let adapter = column.to_adapter().expect("complete column");
    assert_eq!(adapter.coordinates(), (5, 6));
    assert_eq!(adapter.state_at(0, OVERWORLD_MIN_Y, 0), Some(9));
    assert_eq!(adapter.state_at(15, 100, 15), Some(4));
    assert_eq!(adapter.state_at(14, 100, 15), Some(1));
    assert_eq!(adapter.biome_at(1, 0, 2), Some(8));
    assert_eq!(adapter.biome_at(0, 0, 0), Some(3));
}
