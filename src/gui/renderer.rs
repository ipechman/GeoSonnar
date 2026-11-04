//! Explicit renderer selection, configured without creating a graphics instance.
//! Software mode selects a CPU adapter, but wgpu still enumerates DXGI adapters
//! first. It is a diagnostic alternative, not proof that a driver fault is fixed
//! or that no hardware driver is touched during adapter discovery.
use eframe::{NativeOptions, Renderer, egui_wgpu, wgpu};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Direct3D12,
    SoftwareDirect3D12,
    OpenGl,
}

fn software_adapter_eligible(
    backend: wgpu::Backend,
    device_type: wgpu::DeviceType,
    surface_compatible: bool,
) -> bool {
    backend == wgpu::Backend::Dx12 && device_type == wgpu::DeviceType::Cpu && surface_compatible
}

/// Does not enumerate adapters or initialize a graphics API. Native rendering
/// only starts when the caller subsequently enters eframe's event loop.
pub fn configure(options: &mut NativeOptions, mode: Mode) {
    options.multisampling = 0;
    options.depth_buffer = 0;
    options.stencil_buffer = 0;
    if mode == Mode::OpenGl {
        options.renderer = Renderer::Glow;
        options.glow_options.vsync = true;
        return;
    }

    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    // Override environment-derived defaults so an unrelated WGPU_BACKEND,
    // validation or compiler setting cannot change this diagnostic mode.
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    setup.instance_descriptor.flags = wgpu::InstanceFlags::empty();
    setup.instance_descriptor.backend_options = wgpu::BackendOptions::default();
    // Use the system shader compiler instead of searching for a possibly
    // incompatible dxcompiler.dll beside another installed application.
    setup
        .instance_descriptor
        .backend_options
        .dx12
        .shader_compiler = wgpu::Dx12Compiler::Fxc;
    setup.power_preference = wgpu::PowerPreference::LowPower;

    if mode == Mode::SoftwareDirect3D12 {
        setup.native_adapter_selector = Some(Arc::new(|adapters, surface| {
            adapters
                .iter()
                .find(|adapter| {
                    let info = adapter.get_info();
                    software_adapter_eligible(
                        info.backend,
                        info.device_type,
                        surface.is_none_or(|surface| adapter.is_surface_supported(surface)),
                    )
                })
                .cloned()
                .ok_or_else(|| {
                    "No surface-compatible Direct3D 12 software adapter is available. Software mode will not silently switch to a hardware adapter.".into()
                })
        }));
    }

    options.renderer = Renderer::Wgpu;
    options.wgpu_options.wgpu_setup = setup.into();
    // In egui-wgpu 0.35 this is AutoVsync with one queued frame, not an
    // uncapped/tearing present mode.
    options.wgpu_options.surface = eframe::SurfaceConfig::LOW_LATENCY;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_direct3d_without_an_opengl_or_vulkan_fallback() {
        assert_eq!(Mode::default(), Mode::Direct3D12);
        let mut options = NativeOptions::default();
        configure(&mut options, Mode::default());
        assert_eq!(options.renderer, Renderer::Wgpu);
        let egui_wgpu::WgpuSetup::CreateNew(setup) = &options.wgpu_options.wgpu_setup else {
            panic!("Configuration must not eagerly create a graphics device");
        };
        assert_eq!(setup.instance_descriptor.backends, wgpu::Backends::DX12);
        assert!(setup.instance_descriptor.flags.is_empty());
        assert!(matches!(
            setup
                .instance_descriptor
                .backend_options
                .dx12
                .shader_compiler,
            wgpu::Dx12Compiler::Fxc
        ));
        assert_eq!(setup.power_preference, wgpu::PowerPreference::LowPower);
        assert!(setup.native_adapter_selector.is_none());
        assert_eq!(
            options.wgpu_options.surface,
            eframe::SurfaceConfig::LOW_LATENCY
        );
        assert_eq!(
            options.wgpu_options.surface.present_mode,
            wgpu::PresentMode::AutoVsync
        );
        assert_eq!(
            (
                options.multisampling,
                options.depth_buffer,
                options.stencil_buffer
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn software_policy_requires_cpu_dx12_and_surface_support() {
        use wgpu::{Backend, DeviceType};
        assert!(software_adapter_eligible(
            Backend::Dx12,
            DeviceType::Cpu,
            true
        ));
        assert!(!software_adapter_eligible(
            Backend::Dx12,
            DeviceType::Cpu,
            false
        ));
        for kind in [
            DeviceType::DiscreteGpu,
            DeviceType::IntegratedGpu,
            DeviceType::VirtualGpu,
            DeviceType::Other,
        ] {
            assert!(!software_adapter_eligible(Backend::Dx12, kind, true));
        }
        for backend in [
            Backend::Vulkan,
            Backend::Gl,
            Backend::Metal,
            Backend::BrowserWebGpu,
        ] {
            assert!(!software_adapter_eligible(backend, DeviceType::Cpu, true));
        }
        let mut options = NativeOptions::default();
        configure(&mut options, Mode::SoftwareDirect3D12);
        let egui_wgpu::WgpuSetup::CreateNew(setup) = &options.wgpu_options.wgpu_setup else {
            panic!("No device should be created")
        };
        assert_eq!(setup.instance_descriptor.backends, wgpu::Backends::DX12);
        let selector = setup.native_adapter_selector.as_ref().unwrap();
        assert!(
            selector(&[], None)
                .unwrap_err()
                .contains("will not silently switch")
        );
    }

    #[test]
    fn opengl_remains_an_explicit_legacy_choice() {
        let mut options = NativeOptions::default();
        configure(&mut options, Mode::OpenGl);
        assert_eq!(options.renderer, Renderer::Glow);
        assert!(options.glow_options.vsync);
    }
}
