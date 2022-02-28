use std::cmp::Ordering;

use crate::{Block, CCoord, Chunk, Dimension, HeightMode, RCoord};

use super::biome::Biome;

pub type Rgba = [u8; 4];

/// Palette can be used to take a block description to produce a colour that it
/// should render to.
pub trait Palette {
    fn pick(&self, block: &Block, biome: Option<Biome>) -> Rgba;
}

pub struct TopShadeRenderer<'a, P: Palette> {
    palette: &'a P,
    height_mode: HeightMode,
}

impl<'a, P: Palette> TopShadeRenderer<'a, P> {
    pub fn new(palette: &'a P, mode: HeightMode) -> Self {
        Self {
            palette,
            height_mode: mode,
        }
    }

    pub fn render<C: Chunk>(&self, chunk: &C, north: Option<&C>) -> [Rgba; 16 * 16] {
        let mut data = [[0, 0, 0, 0]; 16 * 16];

        let y_range = chunk.y_range();

        for z in 0..16 {
            for x in 0..16 {
                let air_height = chunk.surface_height(x, z, self.height_mode);
                let block_height = (air_height - 1).max(y_range.start);

                let colour = self.drill_for_colour(x, block_height, z, chunk, y_range.start);

                let north_air_height = match z {
                    // if top of chunk, get height from the chunk above.
                    0 => north
                        .map(|c| c.surface_height(x, 15, self.height_mode))
                        .unwrap_or(block_height),
                    z => chunk.surface_height(x, z - 1, self.height_mode),
                };
                let colour = top_shade_colour(colour, air_height, north_air_height);

                data[z * 16 + x] = colour;
            }
        }

        data
    }

    /// Drill for colour. Starting at y_start, make way down the column until we
    /// have an opaque colour to return. This tackles things like transparency.
    fn drill_for_colour<C: Chunk>(
        &self,
        x: usize,
        y_start: isize,
        z: usize,
        chunk: &C,
        y_min: isize,
    ) -> Rgba {
        let mut y = y_start;
        let mut colour = [0, 0, 0, 0];

        while colour[3] != 255 && y >= y_min {
            let current_biome = chunk.biome(x, y, z);
            let current_block = chunk.block(x, y, z);

            if let Some(current_block) = current_block {
                match current_block {
                    block if is_airy(block) => {
                        y -= 1;
                    }
                    // TODO: Can potentially optimize this for ocean floor using
                    // heightmaps.
                    block if is_watery(block) => {
                        let mut block_colour = self.palette.pick(current_block, current_biome);
                        let water_depth = water_depth(x, y, z, chunk, y_min);
                        let alpha = water_depth_to_alpha(water_depth);

                        block_colour[3] = alpha as u8;

                        colour = a_over_b_colour(colour, block_colour);
                        y -= water_depth;
                    }
                    _ => {
                        let block_colour = self.palette.pick(current_block, current_biome);
                        colour = a_over_b_colour(colour, block_colour);
                        y -= 1;
                    }
                }
            } else {
                return colour;
            }
        }

        colour
    }
}

/// Blocks that are considered as if they are water when determining colour.
fn is_watery(block: &Block) -> bool {
    matches!(
        block.name(),
        "minecraft:water"
            | "minecraft:bubble_column"
            | "minecraft:kelp"
            | "minecraft:kelp_plant"
            | "minecraft:sea_grass"
            | "minecraft:tall_seagrass"
    )
}

/// Blocks that are considered as if they are air when determining colour.
fn is_airy(block: &Block) -> bool {
    matches!(block.name(), "minecraft:air" | "minecraft:cave_air")
}

/// Convert `water_depth` meters of water to an approximate opacity
fn water_depth_to_alpha(water_depth: isize) -> u8 {
    // Water will absorb a fraction of the light per unit depth. So if we say
    // that every metre of water absorbs half the light going through it, then 2
    // metres would absorb 3/4, 3 metres would absorb 7/8 etc.
    //
    // Since RGB is not linear, we can make a very rough approximation of this
    // fractional behavior with a linear equation in RBG space. This pretends
    // water absorbs quadratically x^2, rather than exponentially e^x.
    //
    // We put a lower limit to make rivers and swamps still have water show up
    // well, and an upper limit so that very deep ocean has a tiny bit of
    // transparency still.
    //
    // This is pretty rather than accurate.

    (180 + 2 * water_depth).min(250) as u8
}

fn water_depth<C: Chunk>(x: usize, mut y: isize, z: usize, chunk: &C, y_min: isize) -> isize {
    let mut depth = 1;
    while y > y_min {
        let block = match chunk.block(x, y, z) {
            Some(b) => b,
            None => return depth,
        };

        if is_watery(block) {
            depth += 1;
        } else {
            return depth;
        }
        y -= 1;
    }
    depth
}

/// Merge two potentially transparent colours, A and B, into one as if colour A
/// was laid on top of colour B.
///
/// See https://en.wikipedia.org/wiki/Alpha_compositing
fn a_over_b_colour(colour: [u8; 4], below_colour: [u8; 4]) -> [u8; 4] {
    let linear = |c: u8| (((c as usize).pow(2)) as f32) / ((255 * 255) as f32);

    let over_component = |ca: u8, aa: u8, cb: u8, ab: u8| {
        let ca = linear(ca);
        let cb = linear(cb);
        let aa = linear(aa);
        let ab = linear(ab);
        let a_out = aa + ab * (1. - aa);
        let linear_out = (ca * aa + cb * ab * (1. - aa)) / a_out;
        (linear_out * 255. * 255.).sqrt() as u8
    };

    let over_alpha = |aa: u8, ab: u8| {
        let aa = linear(aa);
        let ab = linear(ab);
        let a_out = aa + ab * (1. - aa);
        (a_out * 255. * 255.).sqrt() as u8
    };

    [
        over_component(colour[0], colour[3], below_colour[0], below_colour[3]),
        over_component(colour[1], colour[3], below_colour[1], below_colour[3]),
        over_component(colour[2], colour[3], below_colour[2], below_colour[3]),
        over_alpha(colour[3], below_colour[3]),
    ]
}

pub trait IntoMap {
    fn into_map(self) -> RegionMap<Rgba>;
}

pub struct RegionMap<T> {
    pub data: Vec<T>,
    pub x: RCoord,
    pub z: RCoord,
}

impl<T: Clone> RegionMap<T> {
    pub fn new(x: RCoord, z: RCoord, default: T) -> Self {
        let mut data: Vec<T> = Vec::new();
        data.resize(16 * 16 * 32 * 32, default);
        Self { data, x, z }
    }

    pub fn chunk_mut(&mut self, x: CCoord, z: CCoord) -> &mut [T] {
        debug_assert!(x.0 >= 0 && z.0 >= 0);

        let len = 16 * 16;
        let begin = (z.0 * 32 + x.0) as usize * len;
        &mut self.data[begin..begin + len]
    }

    pub fn chunk(&self, x: CCoord, z: CCoord) -> &[T] {
        debug_assert!(x.0 >= 0 && z.0 >= 0);

        let len = 16 * 16;
        let begin = (z.0 * 32 + x.0) as usize * len;
        &self.data[begin..begin + len]
    }
}

pub fn render_region<P: Palette, C: Chunk + std::fmt::Debug>(
    x: RCoord,
    z: RCoord,
    dimension: Dimension<C>,
    renderer: TopShadeRenderer<P>,
) -> RegionMap<Rgba> {
    let mut map = RegionMap::new(x, z, [0u8; 4]);

    let region = match dimension.region(x, z) {
        Some(r) => r,
        None => return map,
    };

    let mut cache: [Option<C>; 32] = Default::default();

    // Cache the last row of chunks from the above region to allow top-shading
    // on region boundaries.
    if let Some(r) = dimension.region(x, RCoord(z.0 - 1)) {
        for (x, entry) in cache.iter_mut().enumerate() {
            *entry = r.chunk(CCoord(x as isize), CCoord(31));
        }
    }

    for z in 0isize..32 {
        for x in 0isize..32 {
            let (x, z) = (CCoord(x), CCoord(z));
            let data = map.chunk_mut(x, z);

            let chunk_data = region.chunk(x, z).map(|chunk| {
                // Get the chunk at the same x coordinate from the cache. This
                // should be the chunk that is directly above the current. We
                // know this because once we have processed this chunk we put it
                // in the cache in the same place. So the next time we get the
                // current one will be when we're processing directly below us.
                //
                // Thanks to the default None value this works fine for the
                // first row or for any missing chunks.
                let north = cache[x.0 as usize].as_ref();

                let res = renderer.render(&chunk, north);
                cache[x.0 as usize] = Some(chunk);
                res
            });

            if let Some(d) = chunk_data {
                data[..].clone_from_slice(&d);
            }
        }
    }

    map
}

/// Apply top-shading to the given colour based on the relative height of the
/// block above it. Darker if the above block is taller, and lighter if it's
/// smaller.
///
/// Technically this function darkens colours, but this is also how Minecraft
/// itself shades maps.
fn top_shade_colour(colour: Rgba, height: isize, shade_height: isize) -> Rgba {
    let shade = match height.cmp(&shade_height) {
        Ordering::Less => 180usize,
        Ordering::Equal => 220,
        Ordering::Greater => 255,
    };
    [
        (colour[0] as usize * shade / 255) as u8,
        (colour[1] as usize * shade / 255) as u8,
        (colour[2] as usize * shade / 255) as u8,
        colour[3],
    ]
}
