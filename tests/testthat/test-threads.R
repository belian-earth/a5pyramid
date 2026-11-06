test_that("a5pyramid_threads() gets, sets and restores", {
  withr::local_options(a5pyramid.threads = NULL)
  withr::local_envvar(A5PYRAMID_THREADS = NA)
  default <- a5pyramid_threads()
  expect_true(is.integer(default) && default >= 1L)

  old <- a5pyramid_threads(3)
  expect_s3_class(old, "a5pyramid_threads")
  expect_null(old$threads)
  expect_identical(a5pyramid_threads(), 3L)
  expect_output(print(old), "default")
  a5pyramid_threads(old)
  expect_identical(a5pyramid_threads(), default)

  expect_identical(with(a5pyramid_threads(2), a5pyramid_threads()), 2L)
  expect_identical(a5pyramid_threads(), default)

  withr::local_envvar(A5PYRAMID_THREADS = "5")
  expect_identical(a5pyramid_threads(), 5L)

  expect_error(a5pyramid_threads(0), class = "a5pyramid_error_input")
  expect_error(a5pyramid_threads(1.5), class = "a5pyramid_error_input")
  expect_error(a5pyramid_threads("2"), class = "a5pyramid_error_input")
})
