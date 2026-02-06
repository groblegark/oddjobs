# Shared libraries: wok issue tracking and git merge queue.

import "oj/wok" {
  const "prefix" { value = "oj" }
  const "check"  { value = "make check" }
}

import "oj/git" {
  const "check" { value = "make check" }
}
