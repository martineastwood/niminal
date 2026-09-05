version       = "0.1.0"
author        = "martin"
description   = "Minimal native coding agent"
license       = "MIT"
srcDir        = "src"
bin           = @["niminal"]

requires "nim >= 2.0.0"
# Uncomment after `nimble publish` for nimgent (local --path in nim.cfg still wins):
# requires "nimgent >= 0.1.0"

task test, "Run the test suite":
  exec "nim c -r --hints:off tests/all_tests.nim"
