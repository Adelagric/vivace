#!/usr/bin/env python3
"""Builds the synthetic `solver-problems` registry and project: a set of
`acme/*` packages arranged so that each unsolvable case of
`harness/steps.sh` (`@stderr`) reaches one branch of Composer's problem
messages (`Rule::getPrettyString`, `Problem::getMissingPackageReason`,
`SolverProblemsException::getPrettyString`).

Outputs (committed):
  fixtures/registry/solver-problems.tar.gz     p2 metadata + stubs for every
                                               absent name (a missing file is
                                               fatal over file://)
  fixtures/projects/solver-problems/composer.json
  fixtures/projects/solver-problems/composer.lock   resolved by Composer
  fixtures/projects/solver-problems/composer.lock.removed   same, with
                                               acme/removed ^1.0 required, resolved
                                               BEFORE acme/removed vanishes from
                                               the registry (locked-only case)

Requires `composer` on PATH (the pinned 2.10.3 phar of the other harnesses).
Usage: fixtures/make-solver-problems.py
"""
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REGISTRY = os.path.join(ROOT, "fixtures", "registry", "solver-problems.tar.gz")
PROJECT = os.path.join(ROOT, "fixtures", "projects", "solver-problems")


def pkg(name, version, **extra):
    ref = ("ref" + version.replace(".", "").replace("-", "")).ljust(40, "0")[:40]
    p = {
        "name": name,
        "description": extra.pop("description", f"{name} test package"),
        "version": version,
        "version_normalized": normalize(version),
        "license": ["MIT"],
        "type": extra.pop("type", "library"),
        "source": {"url": f"https://example.invalid/{name}.git", "type": "git", "reference": ref},
        "dist": {"url": f"https://example.invalid/{name}/{version}.zip", "type": "zip", "shasum": "", "reference": ref},
        "time": "2026-01-01T00:00:00+00:00",
    }
    p.update(extra)
    return p


def normalize(version):
    if version.startswith("dev-"):
        return version
    if version.endswith("-dev"):
        base = version[: -len("-dev")].replace(".x", ".9999999")
        parts = base.split(".")
        while len(parts) < 4:
            parts.append("9999999" if parts[-1] == "9999999" else "0")
        return ".".join(parts) + "-dev"
    stab = ""
    if "-" in version:
        version, stab = version.split("-", 1)
        stab = "-" + stab.replace("beta", "beta").replace("alpha", "alpha").replace("RC", "RC")
    parts = version.split(".")
    while len(parts) < 4:
        parts.append("0")
    return ".".join(parts) + stab


# --- the packages, grouped by the branch they exercise ---------------------
PACKAGES = {
    # PACKAGE_REQUIRES → missing target ("could not be found in any version")
    "acme/missing-dep": [pkg("acme/missing-dep", "1.0.0", require={"acme/nope": "^1.0"})],
    # ROOT_REQUIRE with only unstable versions ("does not match your minimum-stability")
    "acme/unstable": [pkg("acme/unstable", "1.0.0-beta1"), pkg("acme/unstable", "dev-main", extra={"branch-alias": {"dev-main": "1.x-dev"}})],
    # PACKAGE_CONFLICT (explicit `conflict`)
    "acme/conflict-a": [pkg("acme/conflict-a", "1.0.0", conflict={"acme/conflict-b": "*"})],
    "acme/conflict-b": [pkg("acme/conflict-b", "1.0.0")],
    # PACKAGE_SAME_NAME: two packages replacing the same name
    "acme/replaces-a": [pkg("acme/replaces-a", "1.0.0", replace={"acme/shared": "self.version"})],
    "acme/replaces-b": [pkg("acme/replaces-b", "1.0.0", replace={"acme/shared": "self.version"})],
    # PACKAGE_ALIAS / INVERSE_ALIAS: a branch alias whose branch needs a missing package
    "acme/aliased": [pkg("acme/aliased", "dev-main", extra={"branch-alias": {"dev-main": "1.x-dev"}}, require={"acme/nope": "^1.0"})],
    # requires chain with a version list to condense (many versions)
    "acme/many": [pkg("acme/many", f"1.{i}.0", require={"acme/nope": "^1.0"}) for i in range(0, 12)],
    # provide lib-nope 1.0 (providers hint on a lib- requirement that no one satisfies)
    "acme/provides-lib": [pkg("acme/provides-lib", "1.0.0", provide={"lib-nope": "1.0.0"}, description="Provides the nope library")],
    # provide ext-nope (providers hint on a missing extension)
    "acme/provides-ext": [pkg("acme/provides-ext", "1.0.0", provide={"ext-nope": "*"}, require={"acme/nope": "^1.0"})],
    # dev extraction failure: a dev package replacing what a non-dev package needs
    "acme/needs-c": [pkg("acme/needs-c", "1.0.0", require={"acme/c": "^1.0"})],
    "acme/replaces-c": [pkg("acme/replaces-c", "1.0.0", replace={"acme/c": "1.0.0"})],
    # locked-only: 1.0.0 is locked, then the package vanishes from the registry
    "acme/removed": [pkg("acme/removed", "1.0.0")],
    "acme/other": [pkg("acme/other", "1.0.0"), pkg("acme/other", "1.1.0")],
    # locked at an unacceptable stability after minimum-stability goes back to stable
    "acme/stab": [pkg("acme/stab", "dev-main")],
    # a backtracking diamond that ends unsolvable (learned rules in the problem)
    "acme/top": [pkg("acme/top", "1.0.0", require={"acme/mid": "^1.0"}), pkg("acme/top", "1.1.0", require={"acme/mid": "^1.1"})],
    "acme/mid": [pkg("acme/mid", "1.0.0", require={"acme/leaf": "^1.0"}), pkg("acme/mid", "1.1.0", require={"acme/leaf": "^2.0"})],
    "acme/leaf": [pkg("acme/leaf", "1.0.0", require={"acme/nope": "^1.0"}), pkg("acme/leaf", "2.0.0", require={"acme/nope": "^1.0"})],
    # requires php ^99 through a dependency
    "acme/needs-php": [pkg("acme/needs-php", "1.0.0", require={"php": "^99"})],
    "acme/needs-ext": [pkg("acme/needs-ext", "1.0.0", require={"ext-nope": "*"})],
}
# Names that are required somewhere but do not exist: empty metadata, so the
# repository answers "no versions" instead of a fatal transport error.
ABSENT = ["acme/nope", "acme/shared", "acme/c"]


def write_registry(reg):
    p2 = os.path.join(reg, "p2")
    for name, versions in PACKAGES.items():
        stable = [v for v in versions if not v["version"].startswith("dev-") and not v["version"].endswith("-dev")]
        dev = [v for v in versions if v not in stable]
        for suffix, subset in (("", stable), ("~dev", dev)):
            path = os.path.join(p2, name + suffix + ".json")
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                json.dump({"minified": "composer/2.0", "packages": {name: subset}}, f, indent=4)
                f.write("\n")
    for name in ABSENT:
        for suffix in ("", "~dev"):
            path = os.path.join(p2, name + suffix + ".json")
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                f.write('{"packages": {"%s": []}}' % name)
    with open(os.path.join(reg, "SNAPSHOT"), "w") as f:
        f.write("synthetic registry built by fixtures/make-solver-problems.py\n")
    with open(os.path.join(reg, "summary.json"), "w") as f:
        f.write('{"filter": {"malware": {}}}\n')


BASE_MANIFEST = {
    "name": "vivace/solver-problems",
    "description": "Solver case: every branch of Composer's unsolvable-set messages (see fixtures/make-solver-problems.py)",
    "minimum-stability": "dev",
    "prefer-stable": True,
    "require": {"acme/other": "^1.0", "acme/stab": "*"},
}


def main():
    tmp = tempfile.mkdtemp(prefix="solver-problems-")
    reg = os.path.join(tmp, "registry")
    os.makedirs(reg)
    write_registry(reg)
    # The lock: Composer resolves the base manifest on the full registry.
    home = os.path.join(tmp, "home")
    os.makedirs(home)
    with open(os.path.join(home, "config.json"), "w") as f:
        json.dump({"repositories": {"snapshot": {"type": "composer", "url": "file://" + reg}, "packagist.org": False}}, f)
    subprocess.run(["bash", "-c", '. "$1/harness/lib/registry.sh" && write_snapshot_packages_json "$2"', "_", ROOT, reg], check=True)
    env = dict(os.environ, COMPOSER_HOME=home, COMPOSER_CACHE_DIR=os.path.join(home, "cache"))
    os.makedirs(PROJECT, exist_ok=True)
    for lock_name, extra_require in (("composer.lock", {}), ("composer.lock.removed", {"acme/removed": "^1.0"})):
        proj = os.path.join(tmp, "proj-" + lock_name)
        os.makedirs(proj)
        manifest = dict(BASE_MANIFEST, require=dict(BASE_MANIFEST["require"], **extra_require))
        with open(os.path.join(proj, "composer.json"), "w") as f:
            json.dump(manifest, f, indent=4)
            f.write("\n")
        subprocess.run(["composer", "update", "--no-install", "--no-scripts", "--no-plugins", "--no-interaction", "--no-audit", "--quiet"], cwd=proj, env=env, check=True)
        shutil.copy(os.path.join(proj, "composer.lock"), os.path.join(PROJECT, lock_name))
        if lock_name == "composer.lock":
            shutil.copy(os.path.join(proj, "composer.json"), os.path.join(PROJECT, "composer.json"))
    # Now drop what the lock refers to: acme/removed vanishes from the
    # registry (the "found in the lock file but not in remote repositories"
    # branch, on `update acme/removed`).
    for suffix in ("", "~dev"):
        with open(os.path.join(reg, "p2", "acme", "removed" + suffix + ".json"), "w") as f:
            f.write('{"packages": {"acme/removed": []}}')
    # Write the registry.
    os.remove(os.path.join(reg, "packages.json"))  # rewritten by the harness with its absolute URL
    with tarfile.open(REGISTRY, "w:gz") as tar:
        for entry in sorted(os.listdir(reg)):
            tar.add(os.path.join(reg, entry), arcname=entry)
    shutil.rmtree(tmp)
    print(f"wrote {REGISTRY} and {PROJECT}/composer.{{json,lock,lock.removed}}")


if __name__ == "__main__":
    main()
