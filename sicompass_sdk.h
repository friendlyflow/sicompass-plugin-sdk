/* sicompass-plugin-sdk — C plugin ABI
 *
 * Generated from the sicompass-sdk Rust crate via cbindgen.
 * DO NOT EDIT by hand: regenerate with `cbindgen --config cbindgen.toml --crate sicompass-sdk --output sicompass_sdk.h`
 */

#ifndef SICOMPASS_SDK_H
#define SICOMPASS_SDK_H

#pragma once

#include <stdint.h>
#include <stddef.h>

/**
 * Skip building a `TrashedTree` snapshot above this size — the OS trash
 * becomes the source of truth for restoration. If the trash no longer has
 * the file at undo time, the undo reports an error.
 */
#define TRASH_SNAPSHOT_LIMIT_BYTES ((4 * 1024) * 1024)

/**
 * Mirror of C's `FfonElement`.
 */
typedef struct FfonElementC {
  uint32_t element_type;
  void *data;
} FfonElementC;

/**
 * Mirror of C's `ProviderListItem` (sdk/include/provider_interface.h:20-23).
 */
typedef struct ProviderListItemC {
  char *label;
  char *data;
} ProviderListItemC;

/**
 * Mirror of C's `SearchResultItem` (sdk/include/provider_interface.h:11-15).
 */
typedef struct SearchResultItemC {
  char *label;
  char *breadcrumb;
  char *nav_path;
} SearchResultItemC;

/**
 * Mirror of C's `ProviderOps` vtable that native plugins export.
 *
 * Field order and types MUST match `ProviderOps` in
 * `sdk/include/provider_interface.h` exactly — `#[repr(C)]` layout is
 * position-based, so any divergence corrupts function-pointer reads.
 */
typedef struct ProviderOpsC {
  const char *name;
  const char *display_name;
  /**
   * `FfonElement** (*fetch)(const char *path, int *outCount)`
   */
  struct FfonElementC **(*fetch)(const char *path, int *out_count);
  /**
   * `bool (*commit)(const char *path, const char *old, const char *new)`
   */
  bool (*commit)(const char *path, const char *old_name, const char *new_name);
  bool (*create_directory)(const char *path, const char *name);
  bool (*create_file)(const char *path, const char *name);
  bool (*delete_item)(const char *path, const char *name);
  /**
   * `bool (*copyItem)(const char *srcDir, const char *srcName, const char *destDir, const char *destName)`
   */
  bool (*copy_item)(const char *src_dir,
                    const char *src_name,
                    const char *dest_dir,
                    const char *dest_name);
  /**
   * `const char** (*getCommands)(int *outCount)`
   */
  const char **(*get_commands)(int *out_count);
  /**
   * `FfonElement* (*handleCommand)(const char *path, const char *command, const char *elementKey, int elementType, char *errorMsg, int errorMsgSize)`
   */
  struct FfonElementC *(*handle_command)(const char *path,
                                         const char *command,
                                         const char *element_key,
                                         int element_type,
                                         char *error_msg,
                                         int error_msg_size);
  /**
   * `ProviderListItem* (*getCommandListItems)(const char *path, const char *command, int *outCount)`
   */
  struct ProviderListItemC *(*get_command_list_items)(const char *path,
                                                      const char *command,
                                                      int *out_count);
  /**
   * `bool (*executeCommand)(const char *path, const char *command, const char *selection)`
   */
  bool (*execute_command)(const char *path, const char *command, const char *selection);
  /**
   * `SearchResultItem* (*collectDeepSearchItems)(const char *rootPath, int *outCount)`
   */
  struct SearchResultItemC *(*collect_deep_search_items)(const char *root_path, int *out_count);
} ProviderOpsC;

#endif  /* SICOMPASS_SDK_H */
