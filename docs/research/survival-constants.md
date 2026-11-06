# Survival constant evidence

Status of every numeric rule in `crates/rustmc-server/src/world/survival.rs` against Java 26.3. Checked 7 November 2026.

No value below has a primary source. Mojang publishes no numeric specification for these rules, the operator-provisioned 26.3 pack data does not contain them, and no real-client or official-server observation has been recorded. The only public evidence found is the community Minecraft Wiki, which is a secondary source and is not pinned to 26.3. "Secondary" therefore means a public statement agrees with the value, not that the value is verified. RustMC makes no parity claim from this table.

Sources, all retrieved 7 November 2026:

- [Hunger](https://minecraft.wiki/w/Hunger)
- [Damage](https://minecraft.wiki/w/Damage)
- [Void](https://minecraft.wiki/w/Void)
- [Health](https://minecraft.wiki/w/Health)
- [Water](https://minecraft.wiki/w/Water)
- [Entity format](https://minecraft.wiki/w/Entity_format)

## Agrees with a secondary source

| Constant | Value | Public statement |
| --- | --- | --- |
| `MAX_HEALTH` | 20 | Health: players normally have 20 maximum health points |
| `MAX_FOOD_LEVEL` | 20 | Hunger: initial food on world creation or respawn is 20 |
| `DEFAULT_SATURATION` | 5 | Hunger: initial saturation is 5 |
| `MAX_SATURATION` | 20 | Hunger: the saturation maximum equals the current food level |
| `MAX_EXHAUSTION` | 4 | Hunger: at 4.0 exhaustion resets and removes 1 saturation, or 1 food at zero saturation |
| `MAX_AIR_SUPPLY` | 300 | Entity format: most mobs hold at most 300 ticks of air; Water: 15 seconds |
| `MIN_AIR_SUPPLY` | -20 | Entity format and Damage: drowning damage occurs at -20, then air resets to 0 |
| `DROWNING_DAMAGE` | 2 | Damage and Water: 2 HP per second. Entity format instead says 1 health, so the sources disagree |
| `STANDARD_HEAL_INTERVAL_TICKS` | 80 | Hunger: every 4 seconds at food 18 or higher |
| `REGEN_EXHAUSTION` | 6 | Hunger: 6.0 exhaustion per 1 HP healed |
| `FAST_HEAL_INTERVAL_TICKS` | 10 | Hunger: saturation boost, 1 HP per 1.5 saturation every 10 ticks, Java only |
| `STARVATION_INTERVAL_TICKS` | 80 | Hunger and Damage: 1 HP every 4 seconds |
| `STARVATION_DAMAGE` | 1 | Hunger and Damage: 1 HP |
| `SUFFOCATION_DAMAGE` | 1 | Damage: 1 HP |
| `FALL_DAMAGE_DISTANCE_THRESHOLD` | 3 | Damage: most mobs take fall damage after falling more than 3 blocks |
| `VOID_DAMAGE` | 4 | Void: 4 HP |

Water cancelling a fall also has only secondary support: Damage lists water among the blocks that reset an entity's change in Y coordinate.

## Conflicts with a secondary source

These are recorded rather than changed, because no primary source settles them. Each needs an observed 26.3 result or an owner decision.

| Rule | RustMC | Public statement |
| --- | --- | --- |
| `VOID_THRESHOLD_Y` | -64 in every dimension | Void: under Y=-128 in the Overworld, Y=-64 in the Nether and End |
| Void cadence | 4 HP every tick | Void: 4 HP every 10 ticks |
| `SUFFOCATION_INTERVAL_TICKS` | 20 | Damage: 1 HP every half second, which is 10 ticks |
| Starvation floor | Damage continues to death on all difficulties | Hunger and Damage: stops at 10 HP on Easy and 1 HP on Normal; only Hard continues |
| Fall damage rounding | Fractional `distance - 3` | Damage: the formula uses a floor function; exact form not confirmed |
| Fast heal amount | Fixed 1 HP and 6 exhaustion | Hunger: 1 HP per 1.5 saturation, so the rate may scale with remaining saturation; not confirmed |

## Unverified

No public statement was found for these.

- Air refill out of water. RustMC refills instantly. Hunger, Water, and Entity format give no rate, and instant refill is a simplification rather than a vanilla claim.
- Per-tick air depletion of exactly 1. Entity format states air is measured in ticks but gives no rate.
- Damage immunity. Damage describes a 10-tick invulnerability window; `apply_damage` has none, so simultaneous sources all apply.
- Ordering of drowning, void, suffocation, and food processing within one tick.
- Whether the food timer resets or holds when a player leaves a healing or starving state.
- `naturalRegeneration` game rule, armor, Protection, Resistance, and `safe_fall_distance` handling, none of which are modelled.
