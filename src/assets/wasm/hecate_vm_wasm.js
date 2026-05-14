export async function createHecateVm() {
    throw new Error(
        "WASM runtime bundle not found. Build and publish hecate_vm_wasm to /assets/wasm/ before using browser mode."
    );
}
