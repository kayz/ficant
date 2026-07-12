#ifndef FICANT_KERNEL_H
#define FICANT_KERNEL_H

#include <stdint.h>

#define FICANT_KERNEL_ABI_VERSION UINT32_C(1)

#if defined(_WIN32)
#if defined(FICANT_KERNEL_BUILD)
#define FICANT_KERNEL_API __declspec(dllexport)
#else
#define FICANT_KERNEL_API __declspec(dllimport)
#endif
#elif defined(__GNUC__)
#define FICANT_KERNEL_API __attribute__((visibility("default")))
#else
#define FICANT_KERNEL_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

FICANT_KERNEL_API uint32_t ficant_kernel_abi_version(void);

#ifdef __cplusplus
}
#endif

#endif
