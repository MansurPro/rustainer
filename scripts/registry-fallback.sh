# Shared by every stage test that needs a real image from a real registry.
#
#   . scripts/registry-fallback.sh
#   try_pull alpine:3.21; case $? in 0) ... ;; 2) rate limited ;; *) broken ;; esac
#   echo "$PULLED"     # the reference that actually worked
#
# Docker Hub allows 100 anonymous pulls per six hours per address. A shared
# CI address has usually spent that quota on somebody else's behalf, and a
# quota is not an incompatibility: a test that reports one as the other is
# exactly the kind of lie this project exists not to tell, and a test that
# quietly skips instead is the same lie with better manners.
#
# mirror.gcr.io is a pull-through cache of Docker Hub - the same manifests,
# the same blobs, the same digests, a different host, no quota. Falling back
# to it keeps these tests about images rather than about quotas, and it
# exercises the pull-through mirror that corporate networks put in front of
# Hub anyway, which is its own compatibility claim worth checking.
#
# Expects `$B` to be the rustainer binary. Sets `$PULLED` and counts
# `$MIRRORED`, so a test can say in its output how much of what it proved
# came from the mirror.

MIRRORED=0
PULLED=

# Did the registry refuse us for quota reasons rather than content ones?
rate_limited() { grep -qE 'TOOMANYREQUESTS|HTTP 429|toomanyrequests' "$1"; }

# Only Docker Hub's own `library/` images have a mirror here; anything with
# a host or a user in its name already lives somewhere else.
mirror_of() {
    case "$1" in
        */*) return 1 ;;
        *) echo "mirror.gcr.io/library/$1" ;;
    esac
}

# 0 = pulled (see $PULLED), 2 = every registry rate-limited us,
# 1 = it genuinely failed (see $PULL_ERR for the message).
PULL_ERR=
try_pull() {
    _ref=$1
    PULLED=$_ref
    _err="${TMPDIR:-/tmp}/rustainer-pull-$$.err"
    if timeout 900 "$B" pull "$_ref" >/dev/null 2>"$_err"; then
        PULL_ERR=
        rm -f "$_err"
        return 0
    fi
    if rate_limited "$_err" && _m=$(mirror_of "$_ref"); then
        if timeout 900 "$B" pull "$_m" >/dev/null 2>"$_err"; then
            PULLED=$_m
            MIRRORED=$((MIRRORED + 1))
            PULL_ERR=
            rm -f "$_err"
            return 0
        fi
    fi
    PULL_ERR=$(tr '\n' ' ' < "$_err" | cut -c1-140)
    rm -f "$_err"
    rate_limited_last=no
    case "$PULL_ERR" in
        *TOOMANYREQUESTS* | *"HTTP 429"* | *toomanyrequests*) return 2 ;;
    esac
    return 1
}

# Report how much of the run came from the mirror. Printed by the tests
# rather than hidden, because "it worked" and "it worked somewhere else"
# are different results.
mirror_note() {
    [ "${MIRRORED:-0}" -gt 0 ] &&
        echo "note: $MIRRORED image(s) came from mirror.gcr.io because Docker Hub rate-limited us"
    return 0
}
