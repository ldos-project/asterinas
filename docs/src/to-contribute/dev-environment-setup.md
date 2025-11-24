# Development Environment Setup

## Build Caches

Asterinas uses several build caches to speed up the compilation process during local development. Understanding and managing these caches is important for maintaining a clean development environment.

### Cache Locations

The build system uses the following cache directories:

1. **Cargo Cache** (`$DOCKER_RUST_CACHE_LOCATION/cargo`)
   - Stores Rust package registry data and compiled dependencies
   - Default location: `.cache/cargo` in the project root
   - Contains downloaded crates and their compiled artifacts

2. **Rustup Cache** (`$DOCKER_RUST_CACHE_LOCATION/rustup`)
   - Stores Rust toolchain components
   - Default location: `.cache/rustup` in the project root
   - Contains Rust compiler versions and associated tools

3. **Build Target Directories**
   - `target/`: Main workspace build artifacts
   - `osdk/target/`: OSDK-specific build artifacts
   - `test/`: Test-related build files
   - `docs/`: Documentation build files

### Configuring Cache Location

You can customize the cache location by setting the `DOCKER_RUST_CACHE_LOCATION` environment variable:

```bash
export DOCKER_RUST_CACHE_LOCATION=/path/to/your/cache
```

This is particularly useful if:
- Your home directory is on an NFS-backed filesystem
- You want to place caches on a faster storage device
- You need to manage disk space on different volumes

### NFS Filesystem Considerations

If you're using a filesystem backed by NFS, you may experience performance issues with the default cache location. To improve performance:

1. Set `DOCKER_RUST_CACHE_LOCATION` to a local (non-NFS) directory:
   ```bash
   export DOCKER_RUST_CACHE_LOCATION=/local/fast/storage/.cache
   ```

2. Ensure the directory exists and has appropriate permissions:
   ```bash
   mkdir -p /local/fast/storage/.cache
   ```

3. When building, the system will automatically use this location for caches

## Cleaning Build Artifacts

The Makefile provides several targets for cleaning build artifacts:

### clean

The `clean` target removes workspace build artifacts:

```bash
make clean
```

This will:
- Clean Asterinas workspace target files (`cargo clean`)
- Clean OSDK workspace target files
- Clean documentation target files (`mdbook clean`)
- Clean test target files
- Uninstall the OSDK binary

**Note**: This command must be run in the same environment as the build (typically inside the Docker container). Running it outside the container may fail if there are permission mismatches.

### clean_cache

The `clean_cache` target removes the Cargo and Rustup caches:

```bash
make clean_cache
```

This will:
- Remove the entire `DOCKER_RUST_CACHE_LOCATION` directory
- Delete all cached Rust dependencies and toolchain components

**Important**: 
- This command should be run from the **host environment**, not inside the Docker container
- If you encounter permission errors, you may need to run it with elevated privileges:
  ```bash
  sudo make clean_cache
  ```
- Running `clean_cache` inside the container is blocked for safety reasons

### clean_all

The `clean_all` target performs a complete cleanup:

```bash
make clean_all
```

This combines both `clean` and `clean_cache`, removing:
- All build artifacts
- All cached dependencies
- All Rust toolchain caches

Use this when you want to start from a completely fresh state.

## Best Practices

1. **Regular Cleaning**: Run `make clean` periodically to remove stale build artifacts

2. **Cache Management**: Only run `make clean_cache` when necessary, as rebuilding the cache can be time-consuming

3. **NFS Performance**: If experiencing slow builds on NFS, configure `DOCKER_RUST_CACHE_LOCATION` to use local storage

4. **Disk Space**: Monitor cache directory sizes, as they can grow large over time:
   ```bash
   du -sh $DOCKER_RUST_CACHE_LOCATION
   ```

5. **Environment Awareness**: Remember to run `clean` inside the container and `clean_cache` on the host

## Troubleshooting

### Permission Errors

If you encounter permission errors when cleaning:

1. For `make clean` errors: Ensure you're running inside the Docker container
2. For `make clean_cache` errors: Try running with `sudo` on the host

### Incomplete Builds

If builds fail with cache-related errors:

1. Try cleaning and rebuilding:
   ```bash
   make clean
   make build
   ```

2. If issues persist, clean the cache:
   ```bash
   make clean_all
   make build
   ```

### Slow NFS Performance

If builds are slow on NFS:

1. Verify your cache location:
   ```bash
   echo $DOCKER_RUST_CACHE_LOCATION
   ```

2. Move caches to local storage as described in the NFS Filesystem Considerations section
