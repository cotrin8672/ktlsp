"""Prepare a new binary-only corpus project; compile and probe via the existing harness."""
import hashlib
import json
import os
import re
import shlex
import shutil
import stat
import subprocess
import sys
import zipfile
from pathlib import Path

assert len(sys.argv) == 3 and sys.argv[2] in ("local", "module"), (
    "usage: binary_java_jar_project.py NEW_TEMP_DIRECTORY local|module")
repo = Path(__file__).resolve().parents[1]
destination = Path(os.path.abspath(sys.argv[1]))
assert repo not in (destination, *destination.parents), "use a disposable directory outside the checkout"
assert not os.path.lexists(destination), "destination exists; refuse overwrite"
for ancestor in destination.parents:
    if os.path.lexists(ancestor):
        info = ancestor.lstat()
        assert not stat.S_ISLNK(info.st_mode) and not (
            getattr(info, "st_file_attributes", 0) & 0x400), f"reparse ancestor: {ancestor}"
if os.name == "nt":
    git_exec = Path(subprocess.check_output(["rtk", "proxy", "git", "--exec-path"], encoding="utf-8").strip())
    assert git_exec.is_absolute(), f"Git exec-path is not absolute: {git_exec}"
    bash = next((parent / "bin/bash.exe" for parent in git_exec.parents
                 if (parent / "bin/bash.exe").is_file()), None)
    assert bash is not None, f"Git Bash missing above Git exec-path: {git_exec}"
    bash_argv = [str(bash), "-o", "igncr"]
else:
    bash = shutil.which("bash")
    assert bash is not None, "bash is required"
    bash_argv = [bash]
project = destination / "projects/library"
fixture = repo / "dev/corpus/fixtures/binary-java-jar/binaryuse"
fields_fixture = fixture.parent / "binaryfields"
generics_fixture = fixture.parent / "binarygenerics"
classes = repo / "crates/ktcore/tests/fixtures/classfile"
names = ("arrayfixture/Factory.class", "arrayfixture/Item.class", "pluginfixture/Person.class",
         "defaultfixture/Overlay.class",
         "gradle8142/JavaCompilerArgumentsBuilder.class", "fieldfixture/FieldOnly.class",
         *(f"genericfixture/{name}.class" for name in ("Item", "Other", "Box", "Child", "Holder")),
         *(f"outerfixture/{name}.class" for name in ("Factory", "Item", "SpecialItem", "Other", "Outer", "Outer$Inner", "Outer$Shadow")))
pins = {entry["file"]: entry for entry in json.loads((classes / "hashes.json").read_text(encoding="utf-8"))}
binary = {}
for name in names:
    raw = (classes / name).read_bytes()
    assert (len(raw), hashlib.sha256(raw).hexdigest()) == (pins[name]["bytes"], pins[name]["sha256"])
    entry = "org/gradle/api/internal/tasks/compile/JavaCompilerArgumentsBuilder.class" if name.startswith("gradle8142/") else name
    binary[entry] = raw

sources = {
    "src/main/java/defaultfixture/JavaDefaults.java": (fixture.parent / "binarydefaults/JavaDefaults.java").read_bytes(),
    "src/main/kotlin/defaultfixture/KotlinDefaults.kt": (fixture.parent / "binarydefaults/KotlinDefaults.kt").read_bytes(),
    "src/main/java/binaryuse/JavaUse.java": (fixture / "JavaUse.java").read_bytes(),
    "src/main/kotlin/binaryuse/KotlinUse.kt": (fixture / "KotlinUse.kt").read_bytes(),
    "src/main/java/binaryfields/JavaFields.java": (fields_fixture / "JavaFields.java").read_bytes(),
    "src/main/kotlin/binaryfields/KotlinFields.kt": (fields_fixture / "KotlinFields.kt").read_bytes(),
    "src/main/java/binarygenerics/JavaGenerics.java": (generics_fixture / "JavaGenerics.java").read_bytes(),
    "src/main/kotlin/binarygenerics/KotlinGenerics.kt": (generics_fixture / "KotlinGenerics.kt").read_bytes(),
    "oracle/Holder.stub.txt": (generics_fixture / "Holder.stub.txt").read_bytes(),
    "oracle/SpecialItem.stub.txt": (generics_fixture / "SpecialItem.stub.txt").read_bytes(),
}
manifest_path = repo / "dev/corpus/binary-java-jar.json"
manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
fields_manifest_path = repo / "dev/corpus/binary-java-fields.json"
fields_manifest = json.loads(fields_manifest_path.read_text(encoding="utf-8"))
generics_manifest_path = repo / "dev/corpus/binary-java-generics.json"
generics_manifest = json.loads(generics_manifest_path.read_text(encoding="utf-8"))
defaults_manifest_path = repo / "dev/corpus/binary-java-defaults.json"
defaults_manifest = json.loads(defaults_manifest_path.read_text(encoding="utf-8"))
assert len(manifest["probes"]) == 12 and len(fields_manifest["probes"]) == 2 and len(generics_manifest["probes"]) == 19
assert len(defaults_manifest["probes"]) == 4
for selected in (manifest, fields_manifest, generics_manifest, defaults_manifest):
    for case in selected["probes"]:
        text = "\n".join(sources[case["source"]].decode("utf-8").splitlines())
        assert hashlib.sha256(text.encode("utf-8")).hexdigest() == case["source_sha256"]
harness_path = repo / "dev/ktlsp-harness.sh"
harness = harness_path.read_text(encoding="utf-8")
start = re.search(r"(?m)^write_library_project\(\) \{$", harness)
end = re.search(r"(?m)^run_library_compile\(\) \{$", harness)
assert start and end and start.start() < end.start(), "anchored library function boundaries missing"
function = harness[start.start():end.start()]
shell = ("set -euo pipefail\nrepo=" + shlex.quote(repo.as_posix()) +
         "\nprojects=" + shlex.quote(project.parent.as_posix()) + "\n" + function + "write_library_project\n")
subprocess.run(["rtk", "proxy", *bash_argv, "-c", shell], cwd=repo, check=True)
for relative, raw in sources.items():
    target = project / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(raw)

if sys.argv[2] == "local":
    jar = project / "libs/array-api.jar"
    dependency = "\ndependencies { implementation files('libs/array-api.jar') }\n"
else:
    repository = destination / "maven-repository"
    jar = repository / "arrayfixture/binary-api/1/binary-api-1.jar"
    dependency = "\nrepositories { maven { url = uri('" + repository.as_uri() + "') } }\n"
    dependency += "dependencies { implementation 'arrayfixture:binary-api:1' }\n"
jar.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(jar, "x") as archive:
    for name, raw in binary.items():
        info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = 0o100644 << 16
        archive.writestr(info, raw)
if sys.argv[2] == "module":
    jar.with_suffix(".pom").write_bytes(
        b'<project xmlns="http://maven.apache.org/POM/4.0.0"><modelVersion>4.0.0</modelVersion>'
        b'<groupId>arrayfixture</groupId><artifactId>binary-api</artifactId><version>1</version></project>\n')
with (project / "build.gradle").open("ab") as output:
    output.write(dependency.encode("utf-8"))

def identity(path):
    raw = path.read_bytes()
    return {"path": path.as_posix(), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}

receipt = {
    "mode": sys.argv[2], "project": project.as_posix(), "binary": identity(jar),
    "module_pom": identity(jar.with_suffix(".pom")) if sys.argv[2] == "module" else None,
    "class_inputs": [identity(classes / name) for name in names],
    "harness": identity(harness_path), "manifest": identity(manifest_path),
    "fields_manifest": identity(fields_manifest_path), "generics_manifest": identity(generics_manifest_path),
    "defaults_manifest": identity(defaults_manifest_path),
    "generics_probes": identity(generics_fixture / "probes.lua"), "bash_argv": bash_argv,
    "library_function_sha256": hashlib.sha256(function.encode("utf-8")).hexdigest(),
    "project_files": [identity(path) for path in sorted(project.rglob("*")) if path.is_file()],
    "native_Gradle_Nvim_JVM_started": False,
}
(destination / "preparation.json").write_text(
    json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(project)
