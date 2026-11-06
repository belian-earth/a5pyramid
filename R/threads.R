#' Get or set the number of threads a5pyramid uses
#'
#' Building a pyramid runs in parallel. `a5pyramid_threads()` reports the
#' number of threads builds use by default, and `a5pyramid_threads(n)` sets
#' it for the session. As with [options()] and [a5R::a5_threads()],
#' setting returns the previous value, so `with(a5pyramid_threads(2), expr)`
#' runs `expr` on two threads and then restores the setting, even if `expr`
#' fails. The `threads` argument of [a5_pyramid()] and [a5_write_pyramid()]
#' overrides it for one call.
#'
#' The default is resolved in this order: the option `a5pyramid.threads`,
#' the environment variable `A5PYRAMID_THREADS`, then every CPU available to
#' the R process (which respects affinity masks and container CPU quotas).
#' The count is independent of [a5R::a5_threads()]. Results do not depend
#' on it.
#'
#' @param n `NULL` (the default) to report the current count, an integer of
#'   at least 1 to set it, or the value of an earlier `a5pyramid_threads(n)`
#'   to restore that setting.
#' @param data The value of `a5pyramid_threads(n)`.
#' @param expr Code to run with that thread count.
#' @param ... Unused.
#' @returns `a5pyramid_threads()`: the current count, an integer.
#'   `a5pyramid_threads(n)`: the previous setting, invisibly, as an
#'   `a5pyramid_threads` object. `with()`: the value of `expr`.
#' @export
#' @examples
#' a5pyramid_threads()
#' old <- a5pyramid_threads(2)
#' a5pyramid_threads()
#' a5pyramid_threads(old) # restore
#'
#' with(a5pyramid_threads(1), a5pyramid_threads())
a5pyramid_threads <- function(n = NULL) {
  if (is.null(n)) {
    return(resolve_threads())
  }
  prev <- structure(list(threads = getOption("a5pyramid.threads")), class = "a5pyramid_threads")
  if (inherits(n, "a5pyramid_threads")) {
    options(a5pyramid.threads = n$threads)
  } else {
    options(a5pyramid.threads = check_count(n))
  }
  invisible(prev)
}

#' @rdname a5pyramid_threads
#' @export
with.a5pyramid_threads <- function(data, expr, ...) {
  on.exit(a5pyramid_threads(data), add = TRUE)
  expr
}

#' @export
print.a5pyramid_threads <- function(x, ...) {
  shown <- if (is.null(x$threads)) "default" else format(x$threads)
  cat("<a5pyramid_threads> previous setting:", shown, "\n")
  invisible(x)
}

#' The thread count for a call: `threads` when given, else the session
#' default.
#' @noRd
resolve_threads <- function(threads = NULL, call = rlang::caller_env()) {
  if (!is.null(threads)) {
    return(check_count(threads, "threads", call = call))
  }
  v <- getOption("a5pyramid.threads")
  if (is.null(v)) {
    env <- Sys.getenv("A5PYRAMID_THREADS", unset = "")
    if (nzchar(env)) v <- suppressWarnings(as.integer(env))
  }
  if (is.null(v) || is.na(v) || v < 1L) {
    v <- max(1L, a5pyramid_available_cpus_rs())
  }
  as.integer(v)
}
