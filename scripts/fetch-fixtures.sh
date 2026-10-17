#!/usr/bin/env bash
# Downloads the pinned fixture Packages that AE3's tests read.
#
#   scripts/fetch-fixtures.sh            fetch whatever is missing or wrong
#   scripts/fetch-fixtures.sh --verify   only check what is there; no network
#
# Files land in $AE3_FIXTURES (default: <repo>/fixtures) as
# <Asset Version>/<Platform>/<file>, the layout the tests expect. Every file
# is checked against the SHA-256 pinned below; a file that is already present
# and matches is not downloaded again. Fixtures are game data: they are never
# committed, and CI caches them keyed on this script's hash.
#
# To add a fixture, append "<version> <Platform> <file> <sha256>" to the list
# at the bottom (`shasum -a 256 <file>` or `sha256sum <file>` gives the hash).
# Platform is Windows, Android or iOS.

set -euo pipefail

cdn=${AE3_FIXTURE_CDN:-https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD}
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
script=$script_dir/$(basename "${BASH_SOURCE[0]}")
root=${AE3_FIXTURES:-$script_dir/../fixtures}
# Git Bash on Windows: accept a native path such as D:\a\swgoh-ae3\fixtures.
if command -v cygpath >/dev/null 2>&1; then
  root=$(cygpath -u "$root")
fi
verify_only=false
case ${1:-} in
  "") ;;
  --verify) verify_only=true ;;
  *) echo "usage: $0 [--verify]" >&2; exit 2 ;;
esac

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# The Platform's CDN path segment: device family, then texture format.
cdn_segment() {
  case $1 in
    Windows) echo Windows/ETC ;;
    Android) echo Android/ETC ;;
    iOS) echo iOS/PVRTC ;;
    *) echo "unknown Platform $1" >&2; return 1 ;;
  esac
}

pins() {
  sed -n '/^# PINNED FIXTURES$/,$p' "$script" | grep -E '^[0-9]+ ' || true
}

mkdir -p "$root"
cd "$root"

# Relative paths only from here on, so a Windows root such as D:\a\... never
# reaches curl's config syntax, where a backslash is an escape.
missing=()
curl_config=$(mktemp)
trap 'rm -f "$curl_config"' EXIT
while read -r version platform file sum; do
  path=$version/$platform/$file
  if [[ -f $path && $(sha256 "$path") == "$sum" ]]; then
    continue
  fi
  missing+=("$path $sum")
  mkdir -p "$version/$platform"
  printf 'url = "%s/%s/%s/%s"\noutput = "%s.part"\n' \
    "$cdn" "$version" "$(cdn_segment "$platform")" "$file" "$path" >> "$curl_config"
done < <(pins)

total=$(pins | wc -l | tr -d '[:space:]')
if (( ${#missing[@]} == 0 )); then
  echo "fixtures: all $total present in $root"
  exit 0
fi
if $verify_only; then
  printf 'missing or wrong: %s\n' "${missing[@]%% *}" >&2
  exit 1
fi

echo "fixtures: downloading ${#missing[@]} of $total into $root"
# A failed transfer is reported by curl; the checks below then name the file.
curl --fail --silent --show-error --location --retry 5 --retry-all-errors \
  --parallel --parallel-max 8 --config "$curl_config" || true

status=0
for entry in "${missing[@]}"; do
  path=${entry% *} sum=${entry##* }
  if [[ ! -f $path.part ]]; then
    echo "error: $path was not downloaded" >&2
    status=1
    continue
  fi
  actual=$(sha256 "$path.part")
  if [[ $actual == "$sum" ]]; then
    mv -f "$path.part" "$path"
  else
    echo "error: $path has SHA-256 $actual, expected $sum" >&2
    rm -f "$path.part"
    status=1
  fi
done
exit $status

# PINNED FIXTURES
# <Asset Version> <Platform> <file> <SHA-256>
100054 Windows ability_char_plugin_echo_lb2.bundle 2b9b8c8070da7807194cede16140a28211500b85369735d058b5df0e5cdc1ce6
100054 Windows abilityui_passive_summon.bundle 41e3594671dba05ef05053f53d3f7efba082024f49b2b9c7c96567fbec2f92a4
100054 Windows audio_dataprimer.wwpkg 3ef4660b44c29b4af81e2234a15757a906dbef46407a571856de10caeecc1719
100054 Windows audio_env_hoth.wwpkg 48ea2ee57d6f1460b09b5fcd1fa0252ca81e39fd596ade43917b5cdac2aad710
100054 Windows audio_evt_chewbacca.wwpkg 6abb409d95e224c29b3e4a75ef842f023c93547a0111accd7d1b2a22ad913fa1
100054 Windows audio_ftue.wwpkg fcfa02715bfaa2cc019111051706d339da38a130042f7622b0858f9f0bbbcbfd
100054 Windows audio_mus_campaign.wwpkg 4b4bc12650f4bf4396d54c83cc54f6710090f930a0ab2ce1b427519c9dc491be
100054 Windows audio_sharedui.wwpkg cf3f690e3689bfc4cde3bc74319f25daf22defd5d6d839a83d888fa7e3ea9abd
100054 Windows audio_ui_datacrons.wwpkg 4bffc064192cde8895f4d2a6b375c6d5a46397cf510d8d933a6c63c3006ff726
100054 Windows audio_vo_placeholder.wwpkg 7ebef50283aa26042ccf42ead4fa32193e0c458c215c3a82e137150df089ba8f
100054 Windows char_bossk_pre.bundle 5bb87bce84131335ae2987e6ebc899ae0fa334f153c6d9cfa542eab124bc3982
100054 Windows charui_bossk.bundle a8e53b87471c69904dcd0ab04a9a01f0536ee9fc1fbc6b9301b3accef279758e
100054 Windows conquest_progress_ladder_view.bundle 16344e710c472b3302c09671bc27c6d0e07f48d12e987ec2bce5af2b30584a73
100054 Windows env_space_crait_01_pre.bundle b907a99535d5553e66f2ff8524a6e465229cf57bd51228183d901666e29babf1
100054 Windows env_tat_dunes_jawa_pre.bundle c96879aaa27243d9ab0b9e6c66a1702a754f4fd30fa39bea824bacfdacb95d8c
100054 Windows guild_icon_blast_small.bundle f46892a81cc1ad1c8f4a7a04fa65170cff3e18ca104bd263a5cd443faa83ade7
100054 Windows manifest.data 459d0dfd10181f464e9993bef8b2a0e53dfe34c1dc07f16287fa517518944d4d
100054 Windows pack_jp_ship_gacha.bundle 079f90680bc6f1aa9e9f0614eda8c10a34ad1c96202f2755365621a7a62df932
100054 Windows relic_rfl_bossk.bundle 8b570ec5e4d25f9e567b3482a1b91aa23b906a6ba4af1468398441c443d8652e
100054 Windows shared_resourcecontainer.bundle 6d9e959c72bc91ef3c7a6561fe4d57ca264538769952dd9c0d014e2658703327
100054 Windows shared_uicontainer.bundle a7ea10019811fb401b218af8924dc515f7b413f6855025b29f4d31c45a56849b
100054 Windows territory_backdrop_hoth_atat.bundle 624c97fda6fdb7ebf33ab527e115a68e4e238580e191aea8604d4ebd75280297
100054 Windows territoryui_view_standard_atlas.bundle e16ded45c79cadf82919453caa6f2ca65dd56373de203686690322ef562ebc6a
100054 Windows ui_holotable_currency_tile.bundle e2f61848054b7b7d362477065fdabce30448aedfe0cc69e2f0470c0759e7c7a6
100054 Windows ui_holotable_xp_tile.bundle ac12329aadac8ccc8ad46afb163aa23fde73c1037199289eb5e92433cb5bbb9b
100054 Windows unit_details_atlas.bundle 718669b08dac94eaf825a8d164771f5cd078e7cab18357f3207249b22bde9b1d
100054 Windows vanity_bossk.bundle 6fc897cccc2737ea45818ba22edc70faae541bc49b5504b641743e0160a61a3f
100054 Android abilityui_passive_summon.bundle 5a58f88487dd1349608d22557729a48e27f222bef68b7727c7155692df10f058
100054 Android char_bossk_pre.bundle 1f649a623b1c192567c8c2423fe69977c8e25ecb77af412b025772cf25f9f371
100054 Android charui_bossk.bundle 889d86a3caf74d27f0f373f5e9fadfb3766a0e438573f3b86d21939f854b5fbb
100054 Android env_space_crait_01_pre.bundle 58ee4f389630e56b1058c91656c45721f9985d2131fd02daed24034cb8a9ea5c
100054 Android env_tat_dunes_jawa_pre.bundle a06a2f33da1463c9950964692c5c7b3716328452810319b580dfc586f83c16a9
100054 Android guild_icon_blast_small.bundle 419220d1c377b791905a2e712e953797ddf798c63c0d8a97cec3092bca9bd55a
100054 Android pack_jp_ship_gacha.bundle fa74d1f15c2decbd8bd64a00c1bee3e4c309d623a7b1dbd1dbea732a33edd9cd
100054 Android relic_rfl_bossk.bundle e5f957a7084991bd4edd8782f7bd39e78b64e224537d847b014f2d9e3ddd7a31
100054 Android territory_backdrop_hoth_atat.bundle d630e3f3107aa0ba75795fa514c4f71499681ab6ec2e41ef58c13eb0707ac561
100054 Android territoryui_view_standard_atlas.bundle 47b9a2e2985d5c3ddd1f49b401e60af41e2be8cd08a1c8fc98c5350cc6e710e6
100054 Android unit_details_atlas.bundle 5a9e53b85ff1294b0690536470c04abb8083f6958d0592f422027021a8cc340b
100054 Android vanity_bossk.bundle 6d379c37a7b5e49e4e4be9782acfa3265dacdd15458f8dc2562679c41aa23c0f
100054 iOS abilityui_passive_summon.bundle 7c46fa17317ed1560572231bb9ee6ab711b143f463d385eadcec60cda9fdb9f3
100054 iOS char_bossk_pre.bundle d321a20472a876097be91602f3377bfe4d5c7c6509cd31d4a262e1f4d883ecfb
100054 iOS charui_bossk.bundle 92fa89f8dcf0e53bbbe56f3d3f732026e979be3855ccddb621866481d2a8fc30
100054 iOS env_space_crait_01_pre.bundle e4a7fa74334a55d1ba803dcfa53ba98f046546ab655c9f70275c7e730739ee3e
100054 iOS env_tat_dunes_jawa_pre.bundle 02e2ed9a7d653dca043f7a55b47cd8ef1bbc73ba737461b55cd08f75a16b433c
100054 iOS guild_icon_blast_small.bundle 6d497a5222c9debaebfdc6d4ca3c8f66104b0e2602fe613b25175c414a12e7e9
100054 iOS pack_jp_ship_gacha.bundle c0f2c9376593b64282d4da267fac5501c0009597371bdc846e264a8dc751c819
100054 iOS relic_rfl_bossk.bundle 788c265339d6e472cbd8ab98a6514e5122e9a4ee1d6fdafb1d7e7cdf5f765658
100054 iOS territory_backdrop_hoth_atat.bundle 0dbce347aff137e8fe688fa9f0dff161c50e507e8e6f60ea743d756fa11544bd
100054 iOS territoryui_view_standard_atlas.bundle 60dccc909ba6448faed5bd64361f44ae05167e2e118e1fa1e4c9c23b953e3e38
100054 iOS unit_details_atlas.bundle 59ecc7a0ce35e409cf4b5cfe858a8c96b8a03f7f778ae14d3e5f89d5f03b7e1c
100054 iOS vanity_bossk.bundle 016a6afa89b1c096f1bd3f223f50d778853593d9dce6051ca960148566c52598
