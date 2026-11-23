class Bbstats < Formula
  desc "Dynamic Island background process & developer project telemetry monitor"
  homepage "https://github.com/F0Rextasy/bbstats"
  version "0.4.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/F0Rextasy/bbstats/releases/download/v0.4.0/bbstats-v0.4.0-macos-arm64.tar.gz"
      sha256 "REPLACE_WITH_ARM64_SHA256"
    else
      url "https://github.com/F0Rextasy/bbstats/releases/download/v0.4.0/bbstats-v0.4.0-macos-x86_64.tar.gz"
      sha256 "REPLACE_WITH_X86_64_SHA256"
    end
  end

  on_linux do
    url "https://github.com/F0Rextasy/bbstats/releases/download/v0.4.0/bbstats-v0.4.0-linux-x86_64.tar.gz"
    sha256 "REPLACE_WITH_LINUX_X86_64_SHA256"
  end

  def install
    bin.install "bbstats"
  end

  test do
    assert_match "bbstats", shell_output("#{bin}/bbstats --version")
  end
end
