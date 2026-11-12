#' Abort with class `a5pyramid_error` and, when `type` is given,
#' `a5pyramid_error_<type>`
#' @noRd
a5pyramid_abort <- function(type, message, ..., call = rlang::caller_env(),
                            .envir = parent.frame()) {
  cli::cli_abort(
    message,
    ...,
    class = c(if (!is.null(type)) paste0("a5pyramid_error_", type), "a5pyramid_error"),
    call = call,
    .envir = .envir
  )
}

#' Evaluate a call into Rust, turning its errors into classed conditions
#' @noRd
rs_call <- function(expr, call = rlang::caller_env()) {
  tryCatch(expr, error = function(e) {
    msg <- conditionMessage(e)
    type <- rs_error_type(msg)
    if (is.null(type)) stop(e)
    rlang::abort(msg, class = c(paste0("a5pyramid_error_", type), "a5pyramid_error"), call = call)
  })
}

#' Condition type for a message from the Rust engine, by its prefix; `NULL`
#' for anything else (an R error raised during the call).
#' @noRd
rs_error_type <- function(msg) {
  switch(sub(":.*$", "", msg),
    "invalid input" = "input",
    "format error" = , "json error" = , "parquet error" = , "arrow error" = "format",
    "io error" = "io",
    "interrupted" = "interrupted",
    "internal error" = , "a5 error" = "internal",
    NULL
  )
}

check_string <- function(x, arg = rlang::caller_arg(x), call = rlang::caller_env()) {
  if (!rlang::is_string(x) || !nzchar(x)) {
    a5pyramid_abort("input", "{.arg {arg}} must be a single non-empty string.", call = call)
  }
  invisible(x)
}

check_count <- function(x, arg = rlang::caller_arg(x), min = 1L, call = rlang::caller_env()) {
  ok <- is.numeric(x) && length(x) == 1L && !is.na(x) && x == trunc(x) && x >= min
  if (!ok) {
    a5pyramid_abort("input", "{.arg {arg}} must be a whole number of at least {min}.", call = call)
  }
  as.integer(x)
}

#' `row_group_size` for the engine: 0 for one tile a row group, else a
#' whole number of at least `tile_size`
#' @noRd
check_row_group_size <- function(x, tile_size, arg = rlang::caller_arg(x), call = rlang::caller_env()) {
  if (is.null(x)) {
    return(0L)
  }
  ok <- is.numeric(x) && length(x) == 1L && !is.na(x) && x == trunc(x) && x >= tile_size &&
    x <= .Machine$integer.max
  if (!ok) {
    a5pyramid_abort("input", "{.arg {arg}} must be {.code NULL} or a whole number of at least {.arg tile_size} ({tile_size}).", call = call)
  }
  as.integer(x)
}

#' Validate a user-supplied bbox: `c(xmin, ymin, xmax, ymax)` in WGS 84,
#' with `xmin > xmax` for a box across the antimeridian.
#' @noRd
check_bbox <- function(bbox, call = rlang::caller_env()) {
  if (is.null(bbox)) {
    return(numeric(0))
  }
  ok <- is.numeric(bbox) && length(bbox) == 4L && !anyNA(bbox)
  if (ok) {
    v <- as.double(bbox)
    ok <- all(abs(v[c(1, 3)]) <= 180) && all(abs(v[c(2, 4)]) <= 90) && v[2] <= v[4]
  }
  if (!ok) {
    a5pyramid_abort("input", c(
      "{.arg bbox} must be {.code c(xmin, ymin, xmax, ymax)} in degrees, with longitudes in -180..180 and latitudes in -90..90.",
      "i" = "For a box across the antimeridian give {.code xmin > xmax}, such as {.code c(179, -1, -179, 1)}."
    ), call = call)
  }
  as.double(bbox)
}
