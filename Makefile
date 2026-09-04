# Three ways to build photomem, because they are not interchangeable.
#
# The difference is `release`'s lto = true and codegen-units = 1: they take the
# binary from 20 MB to 11 MB and they are also the two settings that make a
# build serial. Measured on one edit to a string constant, sixteen threads
# either way: 111s release, 10s quick, 1s check. Iterate with `quick`, ship
# `release`.

.DEFAULT_GOAL := quick
.PHONY: release quick check

# The shipped binary: small and slow to build. What install.sh and the release
# CI produce.
release:
	cargo build --release

# The same optimisations without the serial parts. A different binary in a
# different directory — target/quick/photomem — so it can never be mistaken
# for something to install.
quick:
	cargo build --profile quick

# Does it compile. No binary comes out of this.
check:
	cargo check --profile quick
