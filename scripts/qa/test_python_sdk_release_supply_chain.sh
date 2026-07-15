#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SDK_DIR="${ROOT_DIR}/sdk/python"
PYTHON_BIN="${PYTHON_BIN:-python3}"

RUNTIME_INPUT="${SDK_DIR}/requirements.txt"
RUNTIME_LOCK="${SDK_DIR}/requirements.lock"
RELEASE_INPUT="${SDK_DIR}/requirements-release.in"
RELEASE_LOCK="${SDK_DIR}/requirements-release.lock"

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

for file in "$RUNTIME_INPUT" "$RUNTIME_LOCK" "$RELEASE_INPUT" "$RELEASE_LOCK" "${SDK_DIR}/pyproject.toml"; do
    [[ -f "$file" ]] || fail "required Python SDK release file missing: $file"
done

"$PYTHON_BIN" - "$SDK_DIR" "$RUNTIME_INPUT" "$RUNTIME_LOCK" "$RELEASE_LOCK" <<'PY'
import pathlib
import re
import sys

try:
    import tomllib
except ModuleNotFoundError:
    try:
        import tomli as tomllib  # type: ignore[no-redef]
    except ModuleNotFoundError as exc:
        raise SystemExit("tomllib or tomli is required to validate pyproject.toml") from exc

sdk_dir = pathlib.Path(sys.argv[1])
runtime_input = pathlib.Path(sys.argv[2])
runtime_lock = pathlib.Path(sys.argv[3])
release_lock = pathlib.Path(sys.argv[4])


def normalize_name(value: str) -> str:
    return value.lower().replace("_", "-")


def requirement_name(value: str) -> str:
    return normalize_name(re.split(r"[<>=!~;\[]", value, maxsplit=1)[0].strip())


def read_requirements(path: pathlib.Path) -> list[str]:
    return [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]


pyproject = tomllib.loads((sdk_dir / "pyproject.toml").read_text(encoding="utf-8"))
project_dependencies = [dependency.strip() for dependency in pyproject["project"]["dependencies"]]
runtime_dependencies = read_requirements(runtime_input)

if project_dependencies != runtime_dependencies:
    raise SystemExit(
        "sdk/python/pyproject.toml dependencies must match sdk/python/requirements.txt"
    )

lock_requirement = re.compile(r"^([A-Za-z0-9_.-]+)==[^\s]+")


def locked_packages(path: pathlib.Path) -> set[str]:
    packages: set[str] = set()
    has_hash = False

    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("--hash=sha256:"):
            has_hash = True
            continue
        if line[0].isspace():
            continue

        match = lock_requirement.match(stripped)
        if not match:
            raise SystemExit(f"{path}:{line_number} must use exact == pins with hashes")
        packages.add(normalize_name(match.group(1)))

    if not has_hash:
        raise SystemExit(f"{path} does not contain sha256 hashes")
    return packages


runtime_locked = locked_packages(runtime_lock)
locked_packages(release_lock)

missing_runtime = sorted(requirement_name(dependency) for dependency in runtime_dependencies)
missing_runtime = [name for name in missing_runtime if name not in runtime_locked]
if missing_runtime:
    raise SystemExit(
        "runtime lockfile is missing top-level SDK dependencies: "
        + ", ".join(missing_runtime)
    )
PY

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/lichen-python-sdk-release.XXXXXX")"
cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

venv_dir="${tmpdir}/venv"
dist_dir="${tmpdir}/dist"

"$PYTHON_BIN" -m venv "$venv_dir"
venv_python="${venv_dir}/bin/python"

export PIP_DISABLE_PIP_VERSION_CHECK=1
"$venv_python" -m pip install --require-hashes -r "$RELEASE_LOCK"
"$venv_python" -m pip install --require-hashes -r "$RUNTIME_LOCK"
"$venv_python" -m build --wheel --no-isolation --outdir "$dist_dir" "$SDK_DIR"

wheel_file="$(find "$dist_dir" -maxdepth 1 -type f -name 'lichen_sdk-*.whl' -print -quit)"
[[ -n "$wheel_file" ]] || fail "Python SDK wheel was not built"

"$venv_python" -m pip install --no-deps "$wheel_file"
"$venv_python" -m pip check
"$venv_python" -m pip_audit --strict -r "$RUNTIME_LOCK"
"$venv_python" - <<'PY'
from importlib.metadata import version

import lichen

assert lichen is not None
assert version("lichen-sdk") == lichen.__version__
PY

printf 'Python SDK release supply-chain checks passed\n'
