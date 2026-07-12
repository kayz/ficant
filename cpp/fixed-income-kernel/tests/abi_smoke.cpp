#include "ficant_kernel.h"

#include <cstdint>

int main() {
    const std::uint32_t observed = ficant_kernel_abi_version();
    return observed == FICANT_KERNEL_ABI_VERSION ? 0 : 1;
}
