/* External glibc diagnostic only; never linked into Nata. */
#include <malloc.h>
#include <stdio.h>
#include <sys/resource.h>
__attribute__((destructor)) static void report(void) {
    struct mallinfo2 m = mallinfo2();
    struct rusage r;
    getrusage(RUSAGE_SELF, &r);
    fprintf(stderr, "allocator arena=%zu mmap=%zu live=%zu free=%zu peak_rss_kib=%ld\n",
            m.arena,m.hblkhd,m.uordblks,m.fordblks,r.ru_maxrss);
    malloc_stats();
}
