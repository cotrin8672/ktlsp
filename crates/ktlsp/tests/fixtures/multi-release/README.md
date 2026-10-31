# Multi-release class fixtures

These small, original classes were compiled with Oracle JDK 21.0.3. Each
`Api.java` has the same public members and a different constant value so tests
can identify the selected class bytes. `Only` exists only in the version 11
directory. `module-info` is a valid version 9 module descriptor.

For each version `N` in 8, 11, 17, and 21, compile `N/Api.java` using
`javac --release N -encoding UTF-8 -d OUT N/Api.java` and copy
`OUT/mr/Api.class` to `N/Api.class`. Compile `11/Only.java` with
`javac --release 11 -d OUT 11/Only.java`; compile `module-info.java` with
`javac --release 9 -d OUT module-info.java`. Copy each resulting class beside
its source.

SHA-256 of the recorded class files:

```text
8/Api.class        804befdc5bfab29f3917b1998c53ebfe950b132c731d2211eaa3422f9811c9de
11/Api.class       0e85edf94229c8cf3699a5739eec14c2bb1f1a881f0692cfd645d453ce6d693c
11/Only.class      9d8c4cb785f9209088e4ff988dc16357164b445a42981152587cf8c8d0f5a36a
17/Api.class       edd211dd45bac3549dfe43658c60f5fe3515c6330edb2efb1a899d6ffe8e27b7
21/Api.class       f6d65e53364553424e450bf65a5776c9039304a965db13ac71aca7949fb84d9a
module-info.class  4b0bde16828d19316dad77a61dff9bd784c3c9e32d68170a1c4da03ac31fd089
```
