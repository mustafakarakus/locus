# Locus — local-first, long-term memory layer for AI coding agents (U-014).
#
# Installs the `locus` CLI and the `locusd` daemon from a source tarball.
# Publish a tagged release first and set the url/sha256 below:
#   git tag v0.1.0 && git push --tags
#   url  = https://github.com/mustafakarakus/locus/archive/refs/tags/v0.1.0.tar.gz
#   sha256 = shasum -a 256 <tarball>
class Locus < Formula
  desc "Local-first, long-term memory layer for AI coding agents"
  homepage "https://github.com/mustafakarakus/locus"
  license "MIT"
  head "https://github.com/mustafakarakus/locus.git", branch: "main"

  version "0.1.0"
  url "https://github.com/mustafakarakus/locus/archive/refs/tags/v#{version}.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  depends_on "rust" => :build

  def install
    # `locus` auto-starts `locusd`, so both must land in the same bin dir.
    system "cargo", "install", "--path", "crates/locus-cli",
           "--root", prefix.to_s, "--bin", "locus", "--locked"
    system "cargo", "install", "--path", "crates/locusd",
           "--root", prefix.to_s, "--bin", "locusd", "--locked"
  end

  test do
    assert_match "Local-first", shell_output("#{bin}/locus --help")
    assert_match "0.1.0", shell_output("#{bin}/locus --version")
  end
end
