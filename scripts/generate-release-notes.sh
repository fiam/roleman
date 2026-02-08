#!/usr/bin/env bash
set -euo pipefail

workspace_root="${WORKSPACE_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$workspace_root"

new_version="${NEW_VERSION:-${1:-}}"
if [[ -z "$new_version" ]]; then
    echo "error: NEW_VERSION is not set (or pass the version as arg 1)" >&2
    exit 1
fi

changelog_file="${CHANGELOG_FILE:-CHANGELOG.md}"
today="$(date +%Y-%m-%d)"
dry_run="${DRY_RUN:-false}"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

breaking_file="$tmp_dir/breaking.txt"
added_file="$tmp_dir/added.txt"
fixed_file="$tmp_dir/fixed.txt"
performance_file="$tmp_dir/performance.txt"
changed_file="$tmp_dir/changed.txt"
docs_file="$tmp_dir/docs.txt"
maintenance_file="$tmp_dir/maintenance.txt"
other_file="$tmp_dir/other.txt"

touch \
    "$breaking_file" \
    "$added_file" \
    "$fixed_file" \
    "$performance_file" \
    "$changed_file" \
    "$docs_file" \
    "$maintenance_file" \
    "$other_file"

previous_tag="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
if [[ -n "$previous_tag" ]]; then
    log_range=("${previous_tag}..HEAD")
else
    log_range=()
fi

while IFS= read -r subject; do
    [[ -z "$subject" ]] && continue

    if [[ "$subject" =~ ^([[:alpha:]]+)(\(([[:alnum:]_.\/-]+)\))?(!)?:[[:space:]]+(.+)$ ]]; then
        type_raw="${BASH_REMATCH[1]}"
        scope="${BASH_REMATCH[3]:-}"
        bang="${BASH_REMATCH[4]:-}"
        description="${BASH_REMATCH[5]}"
        type="$(printf '%s' "$type_raw" | tr '[:upper:]' '[:lower:]')"

        if [[ -n "$scope" ]]; then
            bullet="- ${description} (${scope})"
        else
            bullet="- ${description}"
        fi

        if [[ -n "$bang" ]]; then
            printf '%s\n' "$bullet" >>"$breaking_file"
            continue
        fi

        case "$type" in
        feat)
            printf '%s\n' "$bullet" >>"$added_file"
            ;;
        fix)
            printf '%s\n' "$bullet" >>"$fixed_file"
            ;;
        perf)
            printf '%s\n' "$bullet" >>"$performance_file"
            ;;
        refactor)
            printf '%s\n' "$bullet" >>"$changed_file"
            ;;
        docs)
            printf '%s\n' "$bullet" >>"$docs_file"
            ;;
        build | chore | ci | style | test | revert)
            printf '%s\n' "$bullet" >>"$maintenance_file"
            ;;
        *)
            printf -- '- %s\n' "$subject" >>"$other_file"
            ;;
        esac
    else
        printf -- '- %s\n' "$subject" >>"$other_file"
    fi
done < <(git log --format='%s' --reverse "${log_range[@]}")

generated_notes="$tmp_dir/generated-notes.md"
printf '### Conventional Commits\n\n' >"$generated_notes"

wrote_sections=0
append_section() {
    local title="$1"
    local path="$2"

    if [[ ! -s "$path" ]]; then
        return
    fi

    printf '#### %s\n' "$title" >>"$generated_notes"
    cat "$path" >>"$generated_notes"
    printf '\n' >>"$generated_notes"
    wrote_sections=1
}

append_section "Breaking" "$breaking_file"
append_section "Added" "$added_file"
append_section "Fixed" "$fixed_file"
append_section "Performance" "$performance_file"
append_section "Changed" "$changed_file"
append_section "Docs" "$docs_file"
append_section "Maintenance" "$maintenance_file"
append_section "Other" "$other_file"

if [[ "$wrote_sections" -eq 0 ]]; then
    printf '#### Other\n- No commits found since the previous release tag.\n\n' >>"$generated_notes"
fi

managed_start="<!-- roleman-conventional-notes:start -->"
managed_end="<!-- roleman-conventional-notes:end -->"

managed_block="$tmp_dir/managed-block.md"
{
    printf '%s\n' "$managed_start"
    cat "$generated_notes"
    printf '%s\n' "$managed_end"
} >"$managed_block"

preview_section="$tmp_dir/release-section-preview.md"
{
    printf '## [%s] - %s\n\n' "$new_version" "$today"
    cat "$generated_notes"
    printf '\n'
} >"$preview_section"

release_section="$tmp_dir/release-section.md"
{
    printf '## [%s] - %s\n\n' "$new_version" "$today"
    cat "$managed_block"
    printf '\n'
} >"$release_section"

if [[ "$dry_run" == "true" ]]; then
    echo "cargo-release dry run; generated release notes preview:"
    echo
    cat "$preview_section"
    exit 0
fi

if [[ ! -f "$changelog_file" ]]; then
    printf '# Changelog\n\n## [Unreleased]\n' >"$changelog_file"
fi

changelog_without_managed="$tmp_dir/changelog-without-managed.md"
awk -v start="$managed_start" -v end="$managed_end" '
BEGIN {
    skipping = 0
}
{
    if (skipping == 0 && $0 == start) {
        skipping = 1
        next
    }
    if (skipping == 1) {
        if ($0 == end) {
            skipping = 0
        }
        next
    }
    print
}
' "$changelog_file" >"$changelog_without_managed"

escaped_version="$(printf '%s\n' "$new_version" | sed -e 's/[][\\/.^$*+?(){}|]/\\&/g')"
if grep -Eq "^##[[:space:]]+\\[?${escaped_version}\\]?([[:space:]]+-.*)?$" "$changelog_without_managed"; then
    updated_changelog="$tmp_dir/changelog-updated.md"
    awk -v version_re="$escaped_version" -v block_path="$managed_block" '
BEGIN {
    in_target = 0
    inserted = 0
}
{
    if ($0 ~ "^##[[:space:]]+\\[?" version_re "\\]?([[:space:]]+-.*)?$") {
        in_target = 1
        print
        next
    }

    if (in_target == 1 && $0 ~ "^##[[:space:]]+") {
        while ((getline line < block_path) > 0) {
            print line
        }
        close(block_path)
        inserted = 1
        in_target = 0
        print
        next
    }

    print
}
END {
    if (in_target == 1 && inserted == 0) {
        while ((getline line < block_path) > 0) {
            print line
        }
        close(block_path)
    }
}
' "$changelog_without_managed" >"$updated_changelog"
    mv "$updated_changelog" "$changelog_file"
    echo "updated ${changelog_file} for v${new_version}"
    exit 0
fi

updated_changelog="$tmp_dir/changelog-updated.md"
awk -v section_path="$release_section" '
BEGIN {
    inserted = 0
}
{
    print
    if (inserted == 0 && $0 ~ /^##[[:space:]]+\[?Unreleased\]?([[:space:]]+-.*)?$/) {
        print ""
        while ((getline line < section_path) > 0) {
            print line
        }
        close(section_path)
        inserted = 1
    }
}
END {
    if (inserted == 0) {
        if (NR > 0) {
            print ""
        }
        print "## [Unreleased]"
        print ""
        while ((getline line < section_path) > 0) {
            print line
        }
        close(section_path)
    }
}
' "$changelog_without_managed" >"$updated_changelog"

mv "$updated_changelog" "$changelog_file"
echo "updated ${changelog_file} for v${new_version}"
