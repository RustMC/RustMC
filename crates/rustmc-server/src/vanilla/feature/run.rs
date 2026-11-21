//! The decoration pass: running each generation step's placed features
//! for one chunk.

use super::*;

impl FeatureData {
    /// Runs one generation step's modeled features for one chunk, drawing
    /// from the stream the step's ordinal gives that chunk, and returns the
    /// number of blocks written.
    ///
    /// Only the ordinals the chunk's 3x3 biome union lists at this step run:
    /// the reference runtime collects exactly that set before it places
    /// anything, so a feature whose only biomes lie outside the neighbourhood
    /// is never attempted here, whatever its own biome filter would say.
    ///
    /// Each feature restarts its stream from the chunk's decoration seed plus
    /// its ordinal and step, so a feature with no runtime here contributes
    /// nothing without moving the seeds of the features around it — which is
    /// also why the ordinals are kept for unmodeled features.
    pub fn decorate_step<S: AsRef<str>>(
        &self,
        target: &mut dyn DecorationTarget,
        step: usize,
        chunk_x: i32,
        chunk_z: i32,
        world_seed: i64,
        region_biomes: &[S],
    ) -> usize {
        let mut written = 0usize;
        for ordinal in self.region_ordinals(step, region_biomes) {
            written += self.decorate_ordinal(target, step, ordinal, chunk_x, chunk_z, world_seed);
        }
        written
    }

    /// Runs the one feature a step's schedule holds at `ordinal` for one
    /// chunk, exactly as [`Self::decorate_step`] would run it, and returns the
    /// number of blocks written.
    ///
    /// Splitting this out is only sound because a feature's stream restarts
    /// from the chunk's decoration seed with its own ordinal and step: no
    /// ordinal can borrow state from the one before it, so running one alone
    /// draws the same sequence as running it inside the step's loop. That is
    /// what lets a measurement attribute a written block to the feature that
    /// placed it.
    pub fn decorate_ordinal(
        &self,
        target: &mut dyn DecorationTarget,
        step: usize,
        ordinal: usize,
        chunk_x: i32,
        chunk_z: i32,
        world_seed: i64,
    ) -> usize {
        let Some(placed) = self.step(step).and_then(|s| s.get(ordinal)) else {
            return 0;
        };
        if !placed.is_modelable() {
            return 0;
        }
        let origin = DecorationPos {
            x: chunk_x << 4,
            y: target.min_y(),
            z: chunk_z << 4,
        };
        let mut random = DecorationRandom::new(world_seed);
        let decoration_seed = random.set_decoration_seed(world_seed, origin.x, origin.z);
        random.set_feature_seed(decoration_seed, ordinal as i32, step as i32);
        self.run_chain(target, placed, 0, origin, &mut random)
    }

    /// Walks one placed feature's decoration list. Each modifier either
    /// filters the candidate out, rewrites it, or multiplies it; the walk is
    /// the documented order, and the draws happen in that order too.
    pub(crate) fn run_chain(
        &self,
        target: &mut dyn DecorationTarget,
        placed: &PlacedFeature,
        index: usize,
        pos: DecorationPos,
        random: &mut DecorationRandom,
    ) -> usize {
        let mut ctx = PlacementContext {
            writable_bounds: target.writable_bounds(),
            target,
            random,
        };
        self.run_chain_step(&mut ctx, placed, index, pos, false)
    }

    pub(super) fn run_chain_step(
        &self,
        ctx: &mut PlacementContext<'_>,
        placed: &PlacedFeature,
        index: usize,
        pos: DecorationPos,
        in_loop: bool,
    ) -> usize {
        if !in_loop
            && !matches!(placed.feature.as_ref(), ConfiguredFeature::Ore { .. })
            && !bounds_overlap(
                pos,
                placed.remaining_write_bounds(index),
                ctx.writable_bounds,
            )
        {
            return 0;
        }
        let Some(modifier) = placed.modifiers.get(index) else {
            let feature = &*placed.feature;
            return match feature {
                ConfiguredFeature::Ore {
                    size,
                    targets,
                    discard_chance_on_air_exposure,
                } => place_ore(
                    ctx.target,
                    ctx.random,
                    &self.tags,
                    *size,
                    targets,
                    *discard_chance_on_air_exposure,
                    pos,
                ),
                ConfiguredFeature::RandomPatch {
                    tries,
                    xz_spread,
                    y_spread,
                    feature: child,
                } => {
                    let child_no_draws = child.remaining_costs_no_draws(0);
                    let child_bounds = child.remaining_write_bounds(0);
                    let mut written = 0;
                    for try_idx in 0..*tries {
                        // The pack's own vegetation states this scatter as an
                        // `offset` of three plateau-free trapezoids, x then y
                        // then z. Two draws per axis, a spread of zero included.
                        let mut axis = |spread: i32| {
                            ctx.random.next_int_bounded(spread + 1)
                                + ctx.random.next_int_bounded(spread + 1)
                                - spread
                        };
                        let dx = axis(*xz_spread);
                        let dy = axis(*y_spread);
                        let dz = axis(*xz_spread);
                        let next = DecorationPos {
                            x: pos.x + dx,
                            y: pos.y + dy,
                            z: pos.z + dz,
                        };
                        let try_in_loop = in_loop || try_idx + 1 < *tries;
                        if (!try_in_loop || child_no_draws)
                            && !bounds_overlap(next, child_bounds, ctx.writable_bounds)
                        {
                            continue;
                        }
                        written += self.run_chain_step(ctx, child, 0, next, try_in_loop);
                    }
                    written
                }
                ConfiguredFeature::SimpleBlock { to_place } => {
                    place_simple_block(ctx.target, ctx.random, &self.tags, to_place, pos)
                }
                ConfiguredFeature::Disk {
                    state_provider,
                    target_predicate,
                    radius,
                    half_height,
                } => place_disk(
                    ctx.target,
                    ctx.random,
                    &self.tags,
                    DiskFeature {
                        state_provider,
                        target_predicate,
                        radius,
                        half_height: *half_height,
                    },
                    pos,
                ),
                ConfiguredFeature::Spring {
                    state,
                    rock_count,
                    hole_count,
                    requires_block_below,
                    valid_blocks,
                } => place_spring(
                    ctx.target,
                    &self.tags,
                    SpringFeature {
                        state,
                        rock_count: *rock_count,
                        hole_count: *hole_count,
                        requires_block_below: *requires_block_below,
                        valid_blocks,
                    },
                    pos,
                ),
                ConfiguredFeature::SimpleRandomSelector { features } => {
                    if features.is_empty() {
                        0
                    } else {
                        let chosen = ctx.random.next_int_bounded(features.len() as i32) as usize;
                        self.run_chain_step(ctx, &features[chosen], 0, pos, in_loop)
                    }
                }
                ConfiguredFeature::RandomSelector { features, default } => {
                    for (child, chance) in features {
                        if ctx.random.next_float() < *chance {
                            return self.run_chain_step(ctx, child, 0, pos, in_loop);
                        }
                    }
                    self.run_chain_step(ctx, default, 0, pos, in_loop)
                }
                ConfiguredFeature::WeightedRandomSelector { features } => {
                    let total: i32 = features.iter().map(|(_, weight)| *weight).sum();
                    if total <= 0 {
                        0
                    } else {
                        let mut pick = ctx.random.next_int_bounded(total);
                        let mut chosen = None;
                        for (child, weight) in features {
                            if pick < *weight {
                                chosen = Some(child);
                                break;
                            }
                            pick -= *weight;
                        }
                        chosen.map_or(0, |child| self.run_chain_step(ctx, child, 0, pos, in_loop))
                    }
                }
                ConfiguredFeature::RandomBooleanSelector {
                    feature_true,
                    feature_false,
                } => {
                    let chosen = if ctx.random.next_bool() {
                        feature_true
                    } else {
                        feature_false
                    };
                    self.run_chain_step(ctx, chosen, 0, pos, in_loop)
                }
                ConfiguredFeature::BlockColumn {
                    direction,
                    allowed_placement,
                    prioritize_tip,
                    layers,
                } => place_block_column(
                    ctx.target,
                    ctx.random,
                    &self.tags,
                    BlockColumnFeature {
                        direction: *direction,
                        allowed_placement,
                        prioritize_tip: *prioritize_tip,
                        layers,
                    },
                    pos,
                ),
                ConfiguredFeature::Unsupported { .. } => 0,
            };
        };
        match modifier {
            PlacementModifier::Count { count } => {
                let attempts = count.sample(ctx.target.min_y(), ctx.target.height(), ctx.random);
                let total = attempts.max(0);
                let mut written = 0;
                for attempt in 0..total {
                    written += self.run_chain_step(
                        ctx,
                        placed,
                        index + 1,
                        pos,
                        in_loop || attempt + 1 < total,
                    );
                }
                written
            }
            PlacementModifier::RarityFilter { chance } => {
                if ctx.random.next_float() < 1.0f32 / (*chance as f32) {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::RandomChance { chance } => {
                if ctx.random.next_float() < *chance {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::InSquare => {
                let mut next = pos;
                next.x += ctx.random.next_int_bounded(16);
                next.z += ctx.random.next_int_bounded(16);
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::Heightmap { heightmap } => {
                let mut next = pos;
                next.y = ctx
                    .target
                    .heightmap_height(*heightmap, pos.x, pos.z, &self.tags);
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::Offset { x, y, z } => {
                let dx = x
                    .sample(ctx.target.min_y(), ctx.target.height(), ctx.random)
                    .clamp(-16, 16);
                let dy = y
                    .sample(ctx.target.min_y(), ctx.target.height(), ctx.random)
                    .clamp(-16, 16);
                let dz = z
                    .sample(ctx.target.min_y(), ctx.target.height(), ctx.random)
                    .clamp(-16, 16);
                let next = DecorationPos {
                    x: pos.x + dx,
                    y: pos.y + dy,
                    z: pos.z + dz,
                };
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::HeightRange { height: provider } => {
                let mut next = pos;
                next.y = provider.sample(ctx.target.min_y(), ctx.target.height(), ctx.random);
                self.run_chain_step(ctx, placed, index + 1, next, in_loop)
            }
            PlacementModifier::Biome => {
                // The filter asks the biome at the candidate position, not at
                // the chunk origin, and it costs no draws.
                let allowed = ctx.target.has_biome(pos.x, pos.y, pos.z, &|biome| {
                    self.biome_lists(biome, &placed.id)
                });
                if allowed {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::BlockPredicateFilter { predicate } => {
                // The predicate reads the terrain around the candidate and
                // costs no draws; an unmodeled shape inside it is gated out by
                // `is_modelable` long before this point.
                if predicate.test(ctx.target, pos, &self.tags) {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::SurfaceRelativeThresholdFilter {
                heightmap,
                min_inclusive,
                max_inclusive,
            } => {
                if heightmap != "OCEAN_FLOOR_WG" {
                    return 0;
                }
                let surface_y = ctx.target.ocean_floor_height(pos.x, pos.z);
                let delta = i64::from(pos.y) - i64::from(surface_y);
                if delta >= i64::from(*min_inclusive) && delta <= i64::from(*max_inclusive) {
                    self.run_chain_step(ctx, placed, index + 1, pos, in_loop)
                } else {
                    0
                }
            }
            PlacementModifier::EnvironmentScan {
                direction_of_search,
                max_steps,
                target_condition,
                allowed_search_condition,
            } => {
                if let Some(allowed) = allowed_search_condition
                    && !allowed.test(ctx.target, pos, &self.tags)
                {
                    return 0;
                }
                let (dx, dy, dz) = direction_of_search.unit_vector();
                let mut cur = pos;
                let mut step = 0;
                while step < *max_steps {
                    if target_condition.test(ctx.target, cur, &self.tags) {
                        return self.run_chain_step(ctx, placed, index + 1, cur, in_loop);
                    }
                    cur.x += dx;
                    cur.y += dy;
                    cur.z += dz;
                    if cur.y < ctx.target.min_y()
                        || cur.y >= ctx.target.min_y() + ctx.target.height()
                    {
                        return 0;
                    }
                    if let Some(allowed) = allowed_search_condition
                        && !allowed.test(ctx.target, cur, &self.tags)
                    {
                        break;
                    }
                    step += 1;
                }
                if target_condition.test(ctx.target, cur, &self.tags) {
                    self.run_chain_step(ctx, placed, index + 1, cur, in_loop)
                } else {
                    0
                }
            }
            // `is_modelable` gates this out before the step runs.
            PlacementModifier::Unsupported { .. } => 0,
        }
    }
}

pub(super) struct PlacementContext<'a> {
    pub(super) target: &'a mut dyn DecorationTarget,
    pub(super) random: &'a mut DecorationRandom,
    pub(super) writable_bounds: Option<(i32, i32, i32, i32)>,
}

pub(super) fn bounds_overlap(
    pos: DecorationPos,
    bounds: Option<(i32, i32, i32, i32)>,
    writable: Option<(i32, i32, i32, i32)>,
) -> bool {
    let (Some((min_dx, max_dx, min_dz, max_dz)), Some((min_wx, max_wx, min_wz, max_wz))) =
        (bounds, writable)
    else {
        return true;
    };
    pos.x + max_dx >= min_wx
        && pos.x + min_dx <= max_wx
        && pos.z + max_dz >= min_wz
        && pos.z + min_dz <= max_wz
}
