#!/usr/bin/env bash

fuzz_toolchain_channel() {
    local fuzz_dir="$1"

    awk -F'"' '
        /^[[:space:]]*channel[[:space:]]*=[[:space:]]*"/ {
            print $2
            exit
        }
    ' "${fuzz_dir}/rust-toolchain.toml"
}

fuzz_stage_seed_corpus() {
    local fuzz_dir="$1"
    local target="$2"
    local temp_root="$3"
    local out_var="$4"

    local seed_dir="${fuzz_dir}/corpus/${target}"
    [[ -d "${seed_dir}" ]] || return 1

    local staged_dir="${temp_root}/${target}"
    mkdir -p "${staged_dir}"

    local copied=0
    while IFS= read -r seed; do
        cp "${seed}" "${staged_dir}/$(basename "${seed}")"
        copied=1
    done < <(find "${seed_dir}" -maxdepth 1 -type f -name '*.seed' | sort)

    [[ "${copied}" -eq 1 ]] || return 1
    printf -v "${out_var}" '%s' "${staged_dir}"
}

fuzz_load_target_options() {
    local fuzz_dir="$1"
    local target="$2"
    local out_var="$3"
    local options_file="${fuzz_dir}/options/${target}.options"

    local -a parsed_options=()
    if [[ -f "${options_file}" ]]; then
        while IFS= read -r line || [[ -n "${line}" ]]; do
            line="${line%%#*}"
            line="${line#"${line%%[![:space:]]*}"}"
            line="${line%"${line##*[![:space:]]}"}"
            [[ -n "${line}" ]] || continue
            parsed_options+=("${line}")
        done < "${options_file}"
    fi

    local -n options_ref="${out_var}"
    options_ref=("${parsed_options[@]}")
}
