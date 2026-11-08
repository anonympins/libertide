# Configuration CMake pour la compilation de sherpa-onnx avec Ninja sous Windows
set(CMAKE_VS_PLATFORM_NAME "x64" CACHE STRING "Plateforme x64 requise par onnxruntime" FORCE)
set(SHERPA_ONNX_ENABLE_TTS ON CACHE BOOL "Activer TTS dans sherpa-onnx" FORCE)
set(SHERPA_ONNX_ENABLE_C_API ON CACHE BOOL "Activer C API dans sherpa-onnx" FORCE)