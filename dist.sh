#!/bin/bash
export XWIN_ARCH="x86,x86_64"
targets=({x86_64,aarch64}-{unknown-linux-musl,apple-darwin} {x86_64,i686}-pc-windows-msvc)

resolved_version="$(cargo metadata --format-version=1 | jq -r '.packages[] | select(.name == "gitpatcher-bin").version')"
echo "Compiling gitpatcher-bin: $resolved_version"

function copy_and_sign() {
    local src_name="$1"
    local final_name="$2" 
    if [ ! -f "$src_name" ]; then
        echo "Missing file: $src_name" >&2;
        exit 1;
    fi
    cp "$src_name" "$final_name"
    echo "Signing file: $final_name"
    if [ -f "$final_name.asc" ]; then
        echo "WARNING: Erasing existing GPG signature" >&2;
        rm "$final_name.asc"
    fi
    gpg --quiet --armor --detach-sign "$final_name" || exit 1
    sha256sum "$final_name" > "$final_name.sha256sum" || exit 1
}

for target in "${targets[@]}"; do
    echo "=== Building target: $target ==="
    suffixes=("")
    compflags=(--release --features static)
    if echo $target | grep -Eq '^\w+-pc-windows'; then
        cargo +stable xwin build --target $target "${compflags[@]}" || exit 1
        suffixes=(".exe" ".pdb")
    else
        cargo +stable zigbuild --target $target "${compflags[@]}" || exit 1
    fi
 
    echo "Finished building: $target"

    dist_dir="dist/gitpatcher-${resolved_version}"
    mkdir -p $dist_dir
    for suffix in "${suffixes[@]}"; do
        copy_and_sign "target/$target/release/gitpatcher$suffix" "${dist_dir}/gitpatcher-${resolved_version}-${target}${suffix}"
    done
done

