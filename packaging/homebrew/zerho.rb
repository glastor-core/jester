# Homebrew Formula for glastor-core/homebrew-tap
class Zerho < Formula
  desc "Zero-Node, 100% Safe Rust frontend dev server and production bundler"
  homepage "https://zerho.xyz"
  version "1.0.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/glastor-core/jester/releases/download/v#{version}/zerho-macos-arm64.tar.gz"
      sha256 "REPLACE_WITH_MACOS_ARM64_SHA256"
    else
      url "https://github.com/glastor-core/jester/releases/download/v#{version}/zerho-macos-x86_64.tar.gz"
      sha256 "REPLACE_WITH_MACOS_X86_64_SHA256"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/glastor-core/jester/releases/download/v#{version}/zerho-linux-x86_64.tar.gz"
      sha256 "REPLACE_WITH_LINUX_X86_64_SHA256"
    end
  end

  def install
    bin.install "zerho"
  end

  test do
    assert_match "zerho #{version}", shell_output("#{bin}/zerho --version")
  end
end
