 ---
 Execution Discipline

To survive context compaction and maintain quality:

 - Use the task list fastidiously. Create a task for each implementation step. Mark tasks in_progress before starting, completed only when cargo test --all
 passes. After completing each task, check TaskList for the next one.
 - Never give up or skip steps. If a step is difficult or produces unexpected compiler errors, debug them fully. Do not comment out tests, skip files, or leave TODOs. Every
  step must be complete and passing before moving to the next.
 - Never take shortcuts. Do not partially migrate a file ("I'll come back to this"). Do not leave old patterns alongside new ones in the same file. When migrating a file,
 convert ALL matching patterns in that file.
 - Complete the full plan completely. A final sweep that all call sites were refactored is mandatory, not optional. Run the grep commands and fix any remaining instances. The work is not done until the verification commands all pass.
 - Run cargo test --all after every step. If tests fail, fix them before proceeding. Do not batch fixes across steps.


