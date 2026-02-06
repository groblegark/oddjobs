 ---
 Execution Discipline

 This is a large refactor touching 40+ files across 6 crates. To survive context compaction and maintain quality:

 - Use the task list fastidiously. Create a task for each implementation step (1-10 above). Mark tasks in_progress before starting, completed only when cargo test --all
 passes. After completing each task, check TaskList for the next one.
 - Never give up or skip steps. If a step is difficult or produces unexpected compiler errors, debug them fully. Do not comment out tests, skip files, or leave TODOs. Every
  step must be complete and passing before moving to the next.
 - Never take shortcuts. Do not partially migrate a file ("I'll come back to this"). Do not leave old patterns alongside new ones in the same file. When migrating a file,
 convert ALL matching patterns in that file.
 - Complete the full plan completely. Step 10 (final sweep) is mandatory, not optional. Run the grep commands and fix any remaining instances. The plan is not done until
 the verification commands all pass.
 - Run cargo test --all after every step. If tests fail, fix them before proceeding. Do not batch fixes across steps.


