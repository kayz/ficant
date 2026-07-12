cmake_minimum_required(VERSION 3.31.6)

project(ficant_fixed_income_kernel VERSION 0.1.0 LANGUAGES CXX)

include(CTest)

add_library(ficant_kernel SHARED src/abi_version.cpp)
target_compile_features(ficant_kernel PUBLIC cxx_std_20)
target_compile_definitions(ficant_kernel PRIVATE FICANT_KERNEL_BUILD)
target_include_directories(
  ficant_kernel
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
set_target_properties(
  ficant_kernel
  PROPERTIES
    CXX_EXTENSIONS OFF
    CXX_SCAN_FOR_MODULES OFF
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
)
target_compile_options(ficant_kernel PRIVATE -Wall -Wextra -Wpedantic -Werror)

if(BUILD_TESTING)
  add_executable(ficant_kernel_abi_smoke tests/abi_smoke.cpp)
  target_compile_features(ficant_kernel_abi_smoke PRIVATE cxx_std_20)
  set_target_properties(
    ficant_kernel_abi_smoke
    PROPERTIES
      CXX_EXTENSIONS OFF
      CXX_SCAN_FOR_MODULES OFF
  )
  target_link_libraries(ficant_kernel_abi_smoke PRIVATE ficant_kernel)
  add_test(NAME ficant_kernel_abi_smoke COMMAND ficant_kernel_abi_smoke)
endif()
