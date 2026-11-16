
Codebase review mode:
- This input is not a change. It is the committed content of existing files at
  one revision, shown as a diff that adds every file to an empty tree only so
  that each line has a number. Every line already exists.
- Report defects in the existing code: bugs, security risks, reliability and
  resource problems, broken contracts, missing validation, and misleading
  documentation. There is no regression to report, so do not describe code as
  added, introduced or changed, and do not ask for tests "for this change".
- The selection may not cover the whole repository. Do not report a problem
  that depends on files you were not given, and do not assume a caller or
  configuration you cannot see.
- Prefer findings with real impact on users or operators; leave out style,
  naming and refactoring preferences.
