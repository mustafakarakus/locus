# Locus — local-first, long-term memory layer for AI coding agents (U-014).
#
# Installs all four Locus binaries from a source tarball.
# Publish a tagged release first and set the url/sha256 below:
#   git tag v0.1.0 && git push --tags
#   url  = https://github.com/mustafakarakus/locus/archive/refs/tags/v0.1.0.tar.gz
#   sha256 = shasum -a 256 <tarball>
class Locus < Formula
  desc "Local-first, long-term memory layer for AI coding agents"
  homepage "https://github.com/mustafakarakus/locus"
  url "https://github.com/mustafakarakus/locus/archive/refs/tags/v0.1.0.tar.gz"
  version "0.1.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"
  head "https://github.com/mustafakarakus/locus.git", branch: "main"

  depends_on "rust" => :build

  def install
    # Companion binaries must land beside `locus` so daemon auto-start and
    # `locus graph --live` preserve the complete CLI feature set.
    system "cargo", "install", "--path", "crates/locus-cli",
           "--root", prefix.to_s, "--bin", "locus", "--locked"
    system "cargo", "install", "--path", "crates/locusd",
           "--root", prefix.to_s, "--bin", "locusd", "--locked"
    system "cargo", "install", "--path", "crates/locus-mcp",
           "--root", prefix.to_s, "--bin", "locus-mcp", "--locked"
    system "cargo", "install", "--path", "crates/locus-viz",
           "--root", prefix.to_s, "--bin", "locus-viz", "--locked"
  end

  test do
    assert_match "Local-first", shell_output("#{bin}/locus --help")
    assert_match "0.1.0", shell_output("#{bin}/locus --version")
    assert_predicate bin/"locusd", :executable?
    assert_predicate bin/"locus-mcp", :executable?
    assert_predicate bin/"locus-viz", :executable?
  end
end
