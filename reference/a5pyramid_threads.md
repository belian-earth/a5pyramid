# Get or set the number of threads a5pyramid uses

Building a pyramid runs in parallel. `a5pyramid_threads()` reports the
number of threads builds use by default, and `a5pyramid_threads(n)` sets
it for the session. As with
[`options()`](https://rdrr.io/r/base/options.html) and
[`a5R::a5_threads()`](https://belian-earth.github.io/a5R/reference/a5_threads.html),
setting returns the previous value, so
`with(a5pyramid_threads(2), expr)` runs `expr` on two threads and then
restores the setting, even if `expr` fails. The `threads` argument of
[`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)
and
[`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md)
overrides it for one call.

## Usage

``` r
a5pyramid_threads(n = NULL)

# S3 method for class 'a5pyramid_threads'
with(data, expr, ...)
```

## Arguments

- n:

  `NULL` (the default) to report the current count, an integer of at
  least 1 to set it, or the value of an earlier `a5pyramid_threads(n)`
  to restore that setting.

- data:

  The value of `a5pyramid_threads(n)`.

- expr:

  Code to run with that thread count.

- ...:

  Unused.

## Value

`a5pyramid_threads()`: the current count, an integer.
`a5pyramid_threads(n)`: the previous setting, invisibly, as an
`a5pyramid_threads` object.
[`with()`](https://rdrr.io/r/base/with.html): the value of `expr`.

## Details

The default is resolved in this order: the option `a5pyramid.threads`,
the environment variable `A5PYRAMID_THREADS`, then every CPU available
to the R process (which respects affinity masks and container CPU
quotas). The count is independent of
[`a5R::a5_threads()`](https://belian-earth.github.io/a5R/reference/a5_threads.html).
Results do not depend on it.

## Examples

``` r
a5pyramid_threads()
#> [1] 4
old <- a5pyramid_threads(2)
a5pyramid_threads()
#> [1] 2
a5pyramid_threads(old) # restore

with(a5pyramid_threads(1), a5pyramid_threads())
#> [1] 1
```
