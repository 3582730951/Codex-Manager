#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import copy
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path
from typing import Any


FRONTMATTER_KEY_PATTERN = re.compile(r"^([A-Za-z0-9_-]+):(.*)$")
BLOCK_SCALAR_MARKERS = {"|", ">", "|-", ">-", "|+", ">+"}


def log(message: str) -> None:
    print(f"[skill-bundle] {message}", flush=True, file=sys.stderr)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Install the locked Codex skill bundle.")
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument(
        "--resolve-heads",
        action="store_true",
        help="Resolve each GitHub source to the current HEAD before installing.",
    )
    return parser.parse_args()


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def reset_dir(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)


def download_repo_tarball(repo: str, ref: str, work_dir: Path) -> Path:
    repo_dir = work_dir / f"{repo.replace('/', '__')}@{ref[:12]}"
    if repo_dir.exists():
        extracted = [candidate for candidate in repo_dir.iterdir() if candidate.is_dir()]
        if len(extracted) != 1:
            raise RuntimeError(f"Unexpected cached tarball layout for {repo}@{ref}")
        return extracted[0]

    url = f"https://codeload.github.com/{repo}/tar.gz/{ref}"
    log(f"Downloading {repo}@{ref[:12]} from {url}")
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "codex-image-builder/1.0"},
    )
    with urllib.request.urlopen(request) as response:
        payload = response.read()

    repo_dir.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as tar:
        try:
            tar.extractall(repo_dir, filter="data")
        except TypeError:
            tar.extractall(repo_dir)

    extracted = [candidate for candidate in repo_dir.iterdir() if candidate.is_dir()]
    if len(extracted) != 1:
        raise RuntimeError(f"Unexpected tarball layout for {repo}@{ref}")
    return extracted[0]


def resolve_head_ref(repo: str) -> str:
    result = subprocess.run(
        ["git", "ls-remote", f"https://github.com/{repo}.git", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    output = result.stdout.strip()
    if not output:
        raise RuntimeError(f"Unable to resolve HEAD for {repo}")
    return output.split()[0]


def resolve_manifest_sources(manifest: dict[str, Any], resolve_heads: bool) -> dict[str, Any]:
    if not resolve_heads:
        return manifest

    resolved_manifest = copy.deepcopy(manifest)
    for source in resolved_manifest["sources"]:
        resolved_ref = resolve_head_ref(source["repo"])
        source["locked_ref"] = source["ref"]
        source["ref"] = resolved_ref
    resolved_manifest["resolution_mode"] = "heads"
    return resolved_manifest


def find_skill_dirs(base_path: Path) -> list[Path]:
    if not base_path.exists():
        raise RuntimeError(f"Skill base path does not exist: {base_path}")

    candidates: list[Path] = []
    for skill_file in sorted(base_path.rglob("SKILL.md")):
        skill_dir = skill_file.parent
        if skill_dir.name.startswith("."):
            continue
        candidates.append(skill_dir)

    accepted: list[Path] = []
    accepted_set: set[Path] = set()
    for skill_dir in sorted(
        candidates,
        key=lambda item: (len(item.relative_to(base_path).parts), str(item.relative_to(base_path))),
    ):
        if any(parent in accepted_set for parent in skill_dir.parents):
            continue
        accepted.append(skill_dir)
        accepted_set.add(skill_dir)
    return accepted


def ensure_path_within_root(path: Path, root: Path, label: str) -> None:
    try:
        path.relative_to(root)
    except ValueError as exc:
        raise RuntimeError(f"{label} resolves outside the repository root: {path}") from exc


def materialize_tree(
    source_dir: Path,
    destination_dir: Path,
    repo_root: Path,
    materialized_symlinks: list[dict[str, str]],
    active_real_dirs: tuple[Path, ...] = (),
) -> None:
    real_source_dir = source_dir.resolve(strict=True)
    if real_source_dir in active_real_dirs:
        raise RuntimeError(f"Symlink cycle detected while copying {source_dir}")

    destination_dir.mkdir(parents=True, exist_ok=True)
    next_active_real_dirs = active_real_dirs + (real_source_dir,)

    for entry in sorted(source_dir.iterdir(), key=lambda item: item.name):
        destination = destination_dir / entry.name

        if entry.is_symlink():
            try:
                resolved = entry.resolve(strict=True)
            except FileNotFoundError as exc:
                raise RuntimeError(f"Broken symlink detected in skill tree: {entry}") from exc
            ensure_path_within_root(resolved, repo_root.resolve(strict=True), "Symlink target")
            materialized_symlinks.append(
                {
                    "path": str(entry),
                    "target": os.readlink(entry),
                    "resolved": str(resolved),
                }
            )
            if resolved.is_dir():
                materialize_tree(
                    resolved,
                    destination,
                    repo_root,
                    materialized_symlinks,
                    next_active_real_dirs,
                )
            elif resolved.is_file():
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(resolved, destination)
            else:
                raise RuntimeError(f"Unsupported symlink target type: {entry} -> {resolved}")
            continue

        if entry.is_dir():
            materialize_tree(entry, destination, repo_root, materialized_symlinks, next_active_real_dirs)
            continue

        if entry.is_file():
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(entry, destination)
            continue

        raise RuntimeError(f"Unsupported filesystem entry in skill tree: {entry}")


def extract_frontmatter(text: str, source_path: Path) -> dict[str, str]:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        raise RuntimeError(f"Missing YAML frontmatter in {source_path}")

    end_index = None
    for index in range(1, len(lines)):
        if lines[index].strip() == "---":
            end_index = index
            break
    if end_index is None:
        raise RuntimeError(f"Unterminated YAML frontmatter in {source_path}")

    frontmatter_lines = lines[1:end_index]
    frontmatter: dict[str, str] = {}
    index = 0
    while index < len(frontmatter_lines):
        line = frontmatter_lines[index]
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            index += 1
            continue

        indent = len(line) - len(line.lstrip(" "))
        if indent != 0:
            raise RuntimeError(f"Unsupported YAML indentation in {source_path}: {line!r}")

        match = FRONTMATTER_KEY_PATTERN.match(line)
        if not match:
            raise RuntimeError(f"Unsupported YAML frontmatter line in {source_path}: {line!r}")

        key, raw_value = match.groups()
        value = raw_value.lstrip()
        if value in BLOCK_SCALAR_MARKERS:
            block_lines: list[str] = []
            index += 1
            while index < len(frontmatter_lines):
                next_line = frontmatter_lines[index]
                next_stripped = next_line.strip()
                if not next_stripped:
                    block_lines.append("")
                    index += 1
                    continue

                next_indent = len(next_line) - len(next_line.lstrip(" "))
                if next_indent <= indent:
                    break

                block_lines.append(next_line[indent + 1 :])
                index += 1
            frontmatter[key] = "\n".join(block_lines).strip()
            continue

        if value == "":
            nested_candidate_lines: list[str] = []
            index += 1
            while index < len(frontmatter_lines):
                next_line = frontmatter_lines[index]
                if not next_line.strip():
                    nested_candidate_lines.append("")
                    index += 1
                    continue
                next_indent = len(next_line) - len(next_line.lstrip(" "))
                if next_indent <= indent:
                    break
                nested_candidate_lines.append(next_line[next_indent:])
                index += 1

            non_empty_lines = [candidate for candidate in nested_candidate_lines if candidate]
            if non_empty_lines and all(FRONTMATTER_KEY_PATTERN.match(candidate) for candidate in non_empty_lines):
                frontmatter[key] = ""
            else:
                frontmatter[key] = "\n".join(nested_candidate_lines).strip()
            continue

        frontmatter[key] = parse_scalar_value(value, source_path, key)
        index += 1

    return frontmatter


def parse_scalar_value(value: str, source_path: Path, key: str) -> str:
    value = value.strip()
    if value[:1] in {'"', "'"}:
        try:
            parsed = ast.literal_eval(value)
        except (SyntaxError, ValueError) as exc:
            raise RuntimeError(f"Invalid quoted YAML scalar for {key} in {source_path}: {value!r}") from exc
        if not isinstance(parsed, str):
            raise RuntimeError(f"Non-string YAML scalar for {key} in {source_path}: {value!r}")
        return parsed.strip()
    return value.strip()


def load_skill_frontmatter(skill_dir: Path) -> dict[str, str]:
    skill_file = skill_dir / "SKILL.md"
    text = skill_file.read_text(encoding="utf-8")
    return extract_frontmatter(text, skill_file)


def normalize_skill_metadata(frontmatter: dict[str, str], source_path: Path) -> dict[str, str]:
    skill_name = frontmatter.get("name", "").strip()
    description = frontmatter.get("description", "").strip()
    if not skill_name:
        raise RuntimeError(f"Missing frontmatter name in {source_path}")
    if not description:
        raise RuntimeError(f"Missing frontmatter description in {source_path}")
    return {
        "skill_name": skill_name,
        "description": description,
    }


def load_skill_metadata(skill_dir: Path) -> dict[str, str]:
    frontmatter = load_skill_frontmatter(skill_dir)
    return normalize_skill_metadata(frontmatter, skill_dir / "SKILL.md")


def manifest_reserved_skill_names(manifest: dict[str, Any]) -> set[str]:
    return {str(name).strip() for name in manifest.get("reserved_system_skill_names", []) if str(name).strip()}


def source_excluded_skill_names(source: dict[str, Any]) -> set[str]:
    return {
        str(name).strip()
        for name in source.get("exclude_frontmatter_names", [])
        if str(name).strip()
    }


def maybe_exclude_skill(
    manifest: dict[str, Any],
    source: dict[str, Any],
    skill_dir: Path,
    metadata: dict[str, str],
) -> bool:
    skill_name = metadata["skill_name"]
    if skill_name in manifest_reserved_skill_names(manifest):
        log(
            "Skipping "
            f"{source['label']}:{skill_dir.name} because frontmatter name {skill_name!r} is reserved by upstream system skills"
        )
        return True

    if skill_name in source_excluded_skill_names(source):
        log(
            "Skipping "
            f"{source['label']}:{skill_dir.name} because frontmatter name {skill_name!r} is excluded by manifest"
        )
        return True

    return False


def resolve_skill_directory_name(directory_name: str, output_skills_dir: Path, source_label: str) -> str:
    destination = output_skills_dir / directory_name
    if not destination.exists():
        return directory_name

    prefixed_name = f"{source_label}-{directory_name}"
    if not (output_skills_dir / prefixed_name).exists():
        log(f"Renaming duplicate skill directory {directory_name} from {source_label} to {prefixed_name}")
        return prefixed_name

    raise RuntimeError(f"Unable to resolve duplicate skill directory name: {directory_name} from {source_label}")


def register_skill_name(
    seen_skill_names: dict[str, str],
    skill_name: str,
    source_label: str,
    directory_name: str,
) -> None:
    owner = f"{source_label}:{directory_name}"
    previous_owner = seen_skill_names.get(skill_name)
    if previous_owner is not None:
        raise RuntimeError(
            f"Duplicate frontmatter skill name detected: {skill_name!r} "
            f"from {owner} conflicts with {previous_owner}"
        )
    seen_skill_names[skill_name] = owner


def copy_skill_dir(
    skill_dir: Path,
    output_skills_dir: Path,
    source: dict[str, Any],
    inventory: list[dict[str, Any]],
    repo_root: Path,
    metadata: dict[str, str],
) -> None:
    original_name = skill_dir.name
    installed_name = resolve_skill_directory_name(original_name, output_skills_dir, source["label"])
    destination = output_skills_dir / installed_name
    if destination.exists():
        raise RuntimeError(f"Duplicate installed skill directory detected: {installed_name}")

    materialized_symlinks: list[dict[str, str]] = []
    materialize_tree(skill_dir, destination, repo_root, materialized_symlinks)
    inventory.append(
        {
            "name": installed_name,
            "directory_name": installed_name,
            "original_name": original_name,
            "skill_name": metadata["skill_name"],
            "description": metadata["description"],
            "source": source["label"],
            "repo": source["repo"],
            "ref": source["ref"],
            "relative_path": str(skill_dir.relative_to(repo_root)).replace("\\", "/"),
            "materialized_symlink_count": len(materialized_symlinks),
        }
    )


def render_frontmatter(frontmatter: dict[str, str] | None) -> str:
    if not frontmatter:
        return ""

    lines = ["---"]
    for key, value in frontmatter.items():
        if any(token in value for token in (":", '"', "\n")):
            safe_value = value.replace('"', '\\"')
            lines.append(f'{key}: "{safe_value}"')
        else:
            lines.append(f"{key}: {value}")
    lines.extend(["---", ""])
    return "\n".join(lines)


def load_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def generate_uipro_skill(
    source: dict[str, Any],
    repo_root: Path,
    output_skills_dir: Path,
    inventory: list[dict[str, Any]],
    seen_skill_names: dict[str, str],
) -> None:
    platform = source.get("platform", "codex")
    config_path = repo_root / "cli" / "assets" / "templates" / "platforms" / f"{platform}.json"
    config = load_json(config_path)

    base_template = load_text(repo_root / "cli" / "assets" / "templates" / "base" / "skill-content.md")
    quick_reference = ""
    if config.get("sections", {}).get("quickReference"):
        quick_reference = "\n" + load_text(repo_root / "cli" / "assets" / "templates" / "base" / "quick-reference.md")

    frontmatter = config.get("frontmatter") or {}
    metadata = normalize_skill_metadata(frontmatter, config_path)
    register_skill_name(seen_skill_names, metadata["skill_name"], source["label"], source["label"])

    skill_content = (
        base_template
        .replace("{{TITLE}}", config["title"])
        .replace("{{DESCRIPTION}}", config["description"])
        .replace("{{SCRIPT_PATH}}", config["scriptPath"])
        .replace("{{SKILL_OR_WORKFLOW}}", config["skillOrWorkflow"])
        .replace("{{QUICK_REFERENCE}}", quick_reference)
    )
    skill_content = render_frontmatter(frontmatter) + skill_content

    original_name = Path(config["folderStructure"]["skillPath"]).name
    installed_name = resolve_skill_directory_name(original_name, output_skills_dir, source["label"])
    skill_dir = output_skills_dir / installed_name
    if skill_dir.exists():
        raise RuntimeError(f"Duplicate generated skill directory detected: {installed_name}")
    skill_dir.mkdir(parents=True, exist_ok=False)
    (skill_dir / config["folderStructure"]["filename"]).write_text(skill_content, encoding="utf-8")

    materialized_symlinks: list[dict[str, str]] = []
    for asset_dir_name in ("data", "scripts"):
        source_dir = repo_root / "cli" / "assets" / asset_dir_name
        if source_dir.exists():
            materialize_tree(source_dir, skill_dir / asset_dir_name, repo_root, materialized_symlinks)

    inventory.append(
        {
            "name": installed_name,
            "directory_name": installed_name,
            "original_name": original_name,
            "skill_name": metadata["skill_name"],
            "description": metadata["description"],
            "source": source["label"],
            "repo": source["repo"],
            "ref": source["ref"],
            "relative_path": "generated-from-cli-assets",
            "materialized_symlink_count": len(materialized_symlinks),
        }
    )


def install_bundle(
    manifest: dict[str, Any],
    output_dir: Path,
    work_dir: Path,
    resolve_heads: bool,
) -> dict[str, Any]:
    manifest = resolve_manifest_sources(manifest, resolve_heads)
    reset_dir(output_dir)
    work_dir.mkdir(parents=True, exist_ok=True)
    output_skills_dir = output_dir / "skills"
    output_skills_dir.mkdir(parents=True, exist_ok=True)

    inventory: list[dict[str, Any]] = []
    seen_skill_names: dict[str, str] = {}

    for source in manifest["sources"]:
        repo_root = download_repo_tarball(source["repo"], source["ref"], work_dir)
        kind = source["kind"]
        if kind == "github-skill-tree":
            base_path = repo_root / source["base_path"]
            skill_dirs = find_skill_dirs(base_path)
            if not skill_dirs:
                raise RuntimeError(f"No skills discovered under {source['repo']}:{source['base_path']}")
            log(f"Installing {len(skill_dirs)} candidate skills from {source['label']}")
            for skill_dir in skill_dirs:
                metadata = load_skill_metadata(skill_dir)
                if maybe_exclude_skill(manifest, source, skill_dir, metadata):
                    continue
                register_skill_name(seen_skill_names, metadata["skill_name"], source["label"], skill_dir.name)
                copy_skill_dir(skill_dir, output_skills_dir, source, inventory, repo_root, metadata)
            continue

        if kind == "uipro-codex-template":
            log("Generating ui-ux-pro-max from upstream Codex template assets")
            generate_uipro_skill(source, repo_root, output_skills_dir, inventory, seen_skill_names)
            continue

        raise RuntimeError(f"Unsupported manifest source kind: {kind}")

    inventory.sort(key=lambda entry: entry["directory_name"])
    symlink_materialization_total = sum(entry["materialized_symlink_count"] for entry in inventory)

    manifest_payload = json.dumps(manifest, ensure_ascii=False, sort_keys=True).encode("utf-8")
    inventory_payload = json.dumps(inventory, ensure_ascii=False, sort_keys=True).encode("utf-8")
    manifest_hash = hashlib.sha256(manifest_payload + b"\n" + inventory_payload).hexdigest()
    manifest_version = f"{manifest['bundle_version']}+{manifest_hash[:12]}"

    output_inventory = {
        "bundle_name": manifest["bundle_name"],
        "bundle_version": manifest["bundle_version"],
        "manifest_version": manifest_version,
        "generated_on": manifest["generated_on"],
        "skill_count": len(inventory),
        "symlink_materialization_total": symlink_materialization_total,
        "skills": inventory,
    }

    (output_dir / "inventory.json").write_text(
        json.dumps(output_inventory, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (output_dir / "manifest.version").write_text(manifest_version + "\n", encoding="utf-8")
    (output_dir / "resolved_manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return output_inventory


def main() -> int:
    args = parse_args()
    manifest = load_json(args.manifest)
    inventory = install_bundle(manifest, args.output_dir, args.work_dir, args.resolve_heads)
    log(
        "Installed "
        f"{inventory['skill_count']} skills into {args.output_dir / 'skills'} "
        f"(manifest {inventory['manifest_version']})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
