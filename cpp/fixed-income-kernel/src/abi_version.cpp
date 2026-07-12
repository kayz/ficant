#include "ficant_kernel.h"

extern "C" uint32_t ficant_kernel_abi_version(void) {
    return FICANT_KERNEL_ABI_VERSION;
}
