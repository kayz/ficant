cmake_minimum_required(VERSION 3.31.6)

project(ficant_fixed_income_kernel VERSION 0.1.0 LANGUAGES CXX)

include(CTest)

add_library(ficant_kernel SHARED
    src/abi_version.cpp
    src/date_utils.cpp
    src/day_count.cpp
    src/bond_math.cpp
    src/kernel_api.cpp
    src/curve_math.cpp
    src/curve_api.cpp
)
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
  # Helper to configure a test executable
  function(ficant_kernel_test TEST_NAME)
    add_executable(${TEST_NAME} tests/${TEST_NAME}.cpp)
    target_compile_features(${TEST_NAME} PRIVATE cxx_std_20)
    set_target_properties(${TEST_NAME} PROPERTIES
      CXX_EXTENSIONS OFF
      CXX_SCAN_FOR_MODULES OFF)
    target_link_libraries(${TEST_NAME} PRIVATE ficant_kernel)
    add_test(NAME ${TEST_NAME} COMMAND ${TEST_NAME})
  endfunction()

  ficant_kernel_test(abi_smoke)
  ficant_kernel_test(test_constants_and_layout)
  ficant_kernel_test(test_calculate_bond_smoke)
  ficant_kernel_test(test_i3_regression)
  ficant_kernel_test(test_curve_interpolation)
  ficant_kernel_test(test_carry_roll)
endif()
