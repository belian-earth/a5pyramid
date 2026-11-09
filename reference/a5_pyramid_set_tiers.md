# Default tiers of a pyramid set

The tiers
[`a5_write_pyramid_set()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set.md)
uses when none are given: from the leaves down, three LODs a tier, each
with shard resolution 12 below its finest LOD, and every remaining LOD
in one tier with shard resolution 0. Tiers that would hold none of the
levels are left out. For leaves at resolution 18 the tiers are LODs 16
to 18 (shard resolution 6), 13 to 15 (3) and 2 to 12 (0).

## Usage

``` r
a5_pyramid_set_tiers(data_resolution, lods = NULL)
```

## Arguments

- data_resolution:

  The leaves' resolution.

- lods:

  The coarser LODs. Default: every resolution from 2 up to
  `data_resolution`.

## Value

A data frame with one row per tier, in ascending LOD order: `lod_min`,
`lod_max`, `shard_resolution` and `tile_size`.

## Examples

``` r
a5_pyramid_set_tiers(18)
#>   lod_min lod_max shard_resolution tile_size
#> 1       2      12                0     16384
#> 2      13      15                3     16384
#> 3      16      18                6     16384
a5_pyramid_set_tiers(16, lods = c(4, 8, 12, 14))
#>   lod_min lod_max shard_resolution tile_size
#> 1       4      10                0     16384
#> 2      11      13                1     16384
#> 3      14      16                4     16384
```
