#!/usr/bin/env bash
# Build the pinned Horizon fork and install its native programs for the panel.
set -euo pipefail
umask 077

usage() {
  cat <<'EOF'
Usage: scripts/install-horizon.sh --data-dir DIR [--debug] [--resources DIR]
       scripts/install-horizon.sh --data-dir DIR --from-build-dir DIR [--resources DIR]

Build the pinned Horizon source, or use binaries already built for this host,
and install horizon-server and horizon-engine under DIR/horizon. The optional
resources directory must have been prepared from your own Minecraft installation.
An existing DIR/horizon is never replaced.
EOF
}

data_dir=""
build_dir=""
resources=""
profile="release"
while (($#)); do
  case "$1" in
    --data-dir|--from-build-dir|--resources)
      if (($# < 2)) || [[ -z "$2" ]]; then usage >&2; exit 2; fi
      case "$1" in
        --data-dir) data_dir="$2" ;;
        --from-build-dir) build_dir="$2" ;;
        --resources) resources="$2" ;;
      esac
      shift 2 ;;
    --debug) profile="debug"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
if [[ -z "$data_dir" ]]; then usage >&2; exit 2; fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
mkdir -p -- "$data_dir"
data_dir="$(cd "$data_dir" && pwd -P)"
destination="$data_dir/horizon"
if [[ -e "$destination" || -L "$destination" ]]; then
  echo "Horizon is already installed at $destination; stop servers and manage updates explicitly." >&2
  exit 1
fi

if [[ -z "$build_dir" ]]; then
  source_dir="$repo_root/vendor/horizon"
  if [[ ! -e "$repo_root/.git" ]]; then
    echo "Horizon source is not included in this package. Clone the panel fork with submodules, or pass --from-build-dir for trusted host-built binaries." >&2
    exit 1
  fi
  if [[ ! -f "$source_dir/.git" && ! -d "$source_dir/.git" ]]; then
    git -C "$repo_root" submodule update --init --recursive -- vendor/horizon
  fi
  expected="$(git -C "$repo_root" rev-parse HEAD:vendor/horizon)"
  actual="$(git -C "$source_dir" rev-parse HEAD)"
  if [[ "$actual" != "$expected" ]] || [[ -n "$(git -C "$source_dir" status --porcelain --untracked-files=no)" ]]; then
    echo "vendor/horizon must be clean and at the revision pinned by this panel branch." >&2
    exit 1
  fi
  build_args=(build --locked)
  [[ "$profile" == release ]] && build_args+=(--release)
  (cd "$source_dir" && cargo "${build_args[@]}" -p horizon-client --no-default-features --bin horizon-client)
  (cd "$source_dir" && cargo "${build_args[@]}" -p horizon-server --bin horizon-server)
  target_dir="${CARGO_TARGET_DIR:-target}"
  [[ "$target_dir" = /* ]] || target_dir="$source_dir/$target_dir"
  build_dir="$target_dir/$profile"
fi

case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) suffix=".exe" ;; *) suffix="" ;; esac
server="$build_dir/horizon-server$suffix"
engine="$build_dir/horizon-engine$suffix"
if [[ ! -f "$engine" ]]; then engine="$build_dir/horizon-client$suffix"; fi
if [[ ! -f "$server" || ! -f "$engine" ]]; then
  echo "Expected horizon-server and horizon-client (or horizon-engine) in $build_dir." >&2
  exit 1
fi

stage="$(mktemp -d "$data_dir/.horizon-install.XXXXXXXX")"
claimed=0
complete=0
cleanup() {
  if [[ "$claimed" == 1 && "$complete" == 0 ]]; then
    echo "Installation was interrupted after creating $destination; inspect that directory before retrying." >&2
  fi
  rm -rf -- "$stage"
}
trap cleanup EXIT
cp -- "$server" "$stage/horizon-server$suffix"
cp -- "$engine" "$stage/horizon-engine$suffix"
chmod 700 "$stage/horizon-server$suffix" "$stage/horizon-engine$suffix"
if ! server_help="$("$stage/horizon-server$suffix" --help)" || ! "$stage/horizon-engine$suffix" --help >/dev/null; then
  echo "The Horizon programs could not run on this host." >&2
  exit 1
fi
if [[ "$server_help" != *"26.3"* ]]; then
  echo "The Horizon supervisor does not identify Minecraft 26.3; build the pinned fork for this panel." >&2
  exit 1
fi

if [[ -n "$resources" ]]; then
  resources="$(cd "$resources" && pwd -P)"
  if [[ -n "$(find "$resources" -type l -print -quit)" ]]; then
    echo "Prepared game files must not contain symbolic links; copy them into a regular directory first." >&2
    exit 1
  fi
  # Horizon validates the version, checksums and completeness before copying.
  # Create resources/ first so its importer never chooses a global install.
  check_dir="$stage/.resource-import"
  mkdir -p "$check_dir/resources"
  HOME="$check_dir" USERPROFILE="$check_dir" "$stage/horizon-server$suffix" --dir "$check_dir" import-resources "$resources"
  mkdir -p "$stage/.local/horizon-client"
  mv "$check_dir/resources" "$stage/.local/horizon-client/26.3"
  rmdir "$check_dir"
fi

# Claim a new installation directory; never merge into or replace an existing
# one. Publish the supervisor last, so a partial copy cannot be started.
if ! mkdir -- "$destination"; then
  echo "Could not create $destination; an installation may have appeared concurrently." >&2
  exit 1
fi
claimed=1
if [[ -d "$stage/.local" ]]; then mv "$stage/.local" "$destination/.local"; fi
mv "$stage/horizon-engine$suffix" "$destination/horizon-engine$suffix"
mv "$stage/horizon-server$suffix" "$destination/horizon-server$suffix"
complete=1
echo "Installed native Horizon programs in $destination"
if [[ -z "$resources" ]]; then
  echo "Prepare your own Minecraft game files and add them to each server's resources/ directory."
fi
