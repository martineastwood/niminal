cmake_minimum_required(VERSION 3.22)
project(niminal VERSION 0.1.0 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

if(APPLE AND NOT DEFINED OPENSSL_ROOT_DIR)
  execute_process(
    COMMAND brew --prefix openssl@3
    OUTPUT_VARIABLE OPENSSL_ROOT_DIR
    OUTPUT_STRIP_TRAILING_WHITESPACE
    RESULT_VARIABLE BREW_OPENSSL)
  if(NOT BREW_OPENSSL EQUAL 0)
    execute_process(
      COMMAND brew --prefix openssl
      OUTPUT_VARIABLE OPENSSL_ROOT_DIR
      OUTPUT_STRIP_TRAILING_WHITESPACE)
  endif()
endif()

set(OPENSSL_USE_STATIC_LIBS TRUE)
find_package(OpenSSL REQUIRED)
find_package(ZLIB REQUIRED)

include(FetchContent)

set(JSON_BuildTests OFF CACHE BOOL "" FORCE)
set(JSON_Install OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
  nlohmann_json
  GIT_REPOSITORY https://github.com/nlohmann/json.git
  GIT_TAG v3.11.3
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(nlohmann_json)

set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_CURL_EXE OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(BUILD_LIBCURL_DOCS OFF CACHE BOOL "" FORCE)
set(BUILD_MISC_DOCS OFF CACHE BOOL "" FORCE)
set(ENABLE_CURL_MANUAL OFF CACHE BOOL "" FORCE)
set(CURL_DISABLE_INSTALL ON CACHE BOOL "" FORCE)
set(HTTP_ONLY ON CACHE BOOL "" FORCE)
set(CURL_USE_OPENSSL ON CACHE BOOL "" FORCE)
set(CURL_USE_LIBPSL OFF CACHE BOOL "" FORCE)
set(USE_NGHTTP2 OFF CACHE BOOL "" FORCE)
set(USE_LIBIDN2 OFF CACHE BOOL "" FORCE)
set(USE_APPLE_IDN OFF CACHE BOOL "" FORCE)
set(CURL_BROTLI OFF CACHE BOOL "" FORCE)
set(CURL_ZSTD OFF CACHE BOOL "" FORCE)
set(CURL_DISABLE_ALTSVC ON CACHE BOOL "" FORCE)
set(ENABLE_ARES OFF CACHE BOOL "" FORCE)
set(ENABLE_WEBSOCKETS OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
  curl
  GIT_REPOSITORY https://github.com/curl/curl.git
  GIT_TAG curl-8_12_1
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(curl)

set(FTXUI_ENABLE_INSTALL OFF CACHE BOOL "" FORCE)
set(FTXUI_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(FTXUI_BUILD_DOCS OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
  ftxui
  GIT_REPOSITORY https://github.com/ArthurSonzogni/FTXUI.git
  GIT_TAG v6.1.9
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(ftxui)

# FTXUI's insert-mode input uses a thin bar cursor that is hard to see in many
# terminals; a block cursor is much easier to spot.
set(_ftxui_input "${ftxui_SOURCE_DIR}/src/ftxui/component/input.cpp")
if(EXISTS "${_ftxui_input}")
  file(READ "${_ftxui_input}" _ftxui_input_src)
  if(_ftxui_input_src MATCHES "focusCursorBarBlinking")
    string(REPLACE "focusCursorBarBlinking" "focusCursorBlockBlinking"
           _ftxui_input_src "${_ftxui_input_src}")
    file(WRITE "${_ftxui_input}" "${_ftxui_input_src}")
  endif()
endif()

add_library(niminal
  src/niminal/http.cpp
  src/niminal/openai.cpp
  src/niminal/agent.cpp)
add_library(niminal::niminal ALIAS niminal)

target_include_directories(niminal PUBLIC
  ${CMAKE_CURRENT_SOURCE_DIR}/include)

target_link_libraries(niminal PUBLIC
  CURL::libcurl
  nlohmann_json::nlohmann_json
  OpenSSL::SSL
  OpenSSL::Crypto
  ZLIB::ZLIB)

if(APPLE)
  target_link_libraries(niminal PUBLIC
    "-framework CoreFoundation"
    "-framework SystemConfiguration")
endif()

target_compile_options(niminal PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_app
  src/app/main.cpp
  src/app/tui.cpp
  src/app/session.cpp
  src/app/config.cpp
  src/app/provider.cpp
  src/app/instructions.cpp
  src/app/workspace.cpp
  src/app/tools.cpp
  src/app/compaction.cpp
  src/app/markdown.cpp
  src/app/models_dev.cpp
  src/app/thinking.cpp
  src/app/prompts.cpp)
target_sources(niminal_app PRIVATE src/app/skills.cpp)
set_target_properties(niminal_app PROPERTIES OUTPUT_NAME niminal)
target_include_directories(niminal_app PRIVATE src/app)
target_link_libraries(niminal_app PRIVATE
  niminal::niminal
  ftxui::component)
target_compile_options(niminal_app PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_workspace_test tests/workspace_test.cpp src/app/workspace.cpp)
target_include_directories(niminal_workspace_test PRIVATE src/app)
target_compile_options(niminal_workspace_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_tools_test
  tests/tools_test.cpp
  src/app/tools.cpp
  src/app/workspace.cpp
  src/app/instructions.cpp
  src/app/config.cpp)
target_include_directories(niminal_tools_test PRIVATE src/app include)
target_link_libraries(niminal_tools_test PRIVATE nlohmann_json::nlohmann_json)
target_compile_options(niminal_tools_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_instructions_test
  tests/instructions_test.cpp
  src/app/instructions.cpp
  src/app/config.cpp)
target_include_directories(niminal_instructions_test PRIVATE src/app)
target_link_libraries(niminal_instructions_test PRIVATE nlohmann_json::nlohmann_json)
target_compile_options(niminal_instructions_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_skills_test tests/skills_test.cpp src/app/skills.cpp src/app/config.cpp)
target_include_directories(niminal_skills_test PRIVATE src/app include)
target_link_libraries(niminal_skills_test PRIVATE nlohmann_json::nlohmann_json)
target_compile_options(niminal_skills_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_prompts_test tests/prompts_test.cpp src/app/prompts.cpp src/app/config.cpp)
target_include_directories(niminal_prompts_test PRIVATE src/app)
target_link_libraries(niminal_prompts_test PRIVATE nlohmann_json::nlohmann_json)
target_compile_options(niminal_prompts_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_config_test tests/config_test.cpp src/app/config.cpp)
target_include_directories(niminal_config_test PRIVATE src/app)
target_link_libraries(niminal_config_test PRIVATE nlohmann_json::nlohmann_json)
target_compile_options(niminal_config_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_provider_test
  tests/provider_test.cpp
  src/app/provider.cpp
  src/app/config.cpp
  src/app/thinking.cpp
  src/app/models_dev.cpp)
target_include_directories(niminal_provider_test PRIVATE src/app include)
target_link_libraries(niminal_provider_test PRIVATE niminal::niminal)
target_compile_options(niminal_provider_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_session_test tests/session_test.cpp src/app/session.cpp)
target_include_directories(niminal_session_test PRIVATE src/app include)
target_link_libraries(niminal_session_test PRIVATE nlohmann_json::nlohmann_json)
target_compile_options(niminal_session_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_compaction_test
  tests/compaction_test.cpp
  src/app/session.cpp
  src/app/compaction.cpp)
target_include_directories(niminal_compaction_test PRIVATE src/app include)
target_link_libraries(niminal_compaction_test PRIVATE niminal::niminal)
target_compile_options(niminal_compaction_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_agent_test tests/agent_test.cpp)
target_link_libraries(niminal_agent_test PRIVATE niminal::niminal)
target_compile_options(niminal_agent_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_markdown_test tests/markdown_test.cpp src/app/markdown.cpp)
target_include_directories(niminal_markdown_test PRIVATE src/app)
target_link_libraries(niminal_markdown_test PRIVATE ftxui::dom)
target_compile_options(niminal_markdown_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_models_dev_test
  tests/models_dev_test.cpp
  src/app/models_dev.cpp
  src/app/config.cpp)
target_include_directories(niminal_models_dev_test PRIVATE src/app include)
target_link_libraries(niminal_models_dev_test PRIVATE niminal::niminal)
target_compile_options(niminal_models_dev_test PRIVATE -Wall -Wextra -Wpedantic)

add_executable(niminal_thinking_test
  tests/thinking_test.cpp
  src/app/thinking.cpp
  src/app/models_dev.cpp
  src/app/config.cpp)
target_include_directories(niminal_thinking_test PRIVATE src/app include)
target_link_libraries(niminal_thinking_test PRIVATE niminal::niminal)
target_compile_options(niminal_thinking_test PRIVATE -Wall -Wextra -Wpedantic)

enable_testing()
add_test(NAME workspace COMMAND niminal_workspace_test)
add_test(NAME tools COMMAND niminal_tools_test)
add_test(NAME instructions COMMAND niminal_instructions_test)
add_test(NAME skills COMMAND niminal_skills_test)
add_test(NAME prompts COMMAND niminal_prompts_test)
add_test(NAME config COMMAND niminal_config_test)
add_test(NAME provider COMMAND niminal_provider_test)
add_test(NAME session COMMAND niminal_session_test)
add_test(NAME compaction COMMAND niminal_compaction_test)
add_test(NAME agent COMMAND niminal_agent_test)
add_test(NAME markdown COMMAND niminal_markdown_test)
add_test(NAME models_dev COMMAND niminal_models_dev_test)
add_test(NAME thinking COMMAND niminal_thinking_test)
