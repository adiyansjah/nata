// Replays real Parquet Snappy pages (dumped by the shim) through Google snappy.
// Output buffer is reused, so this measures decode speed, not allocation.
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>
#include <snappy.h>

int main(int argc, char** argv) {
  FILE* f = fopen(argv[1], "rb");
  struct Blk { std::vector<char> in; uint32_t out; };
  std::vector<Blk> blks; size_t total_out = 0, total_in = 0, max_out = 0;
  uint32_t hdr[2];
  while (fread(hdr, 4, 2, f) == 2) {
    Blk b; b.in.resize(hdr[0]); b.out = hdr[1];
    if (fread(b.in.data(), 1, hdr[0], f) != hdr[0]) break;
    total_in += hdr[0]; total_out += hdr[1]; if (hdr[1] > max_out) max_out = hdr[1];
    blks.push_back(std::move(b));
  }
  std::vector<char> out(max_out + 64);
  double best = 1e18;
  for (int it = 0; it < 40; ++it) {
    auto t0 = std::chrono::steady_clock::now();
    for (auto& b : blks) {
      if (!snappy::RawUncompress(b.in.data(), b.in.size(), out.data())) { fprintf(stderr, "bad block\n"); return 1; }
    }
    double ms = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
    if (ms < best) best = ms;
  }
  printf("%zu blocks, %.1f MB in, %.1f MB out: best %.1f ms = %.2f GB/s\n", blks.size(), total_in/1e6, total_out/1e6, best, total_out/best/1e6);
}
