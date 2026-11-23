// FreeCam Phone DirectShow filter registration, built on softcam (MIT).
// Same structure as softcam's own softcam.cpp but with our name and CLSID.
#include <olectl.h>
#include <initguid.h>

#include <softcamcore/DShowSoftcam.h>

// {3AD27E84-C813-4ACB-8580-F20BAFB1B524}
DEFINE_GUID(CLSID_FreeCamPhone,
0x3ad27e84, 0xc813, 0x4acb, 0x85, 0x80, 0xf2, 0x0b, 0xaf, 0xb1, 0xb5, 0x24);

namespace {

const wchar_t FILTER_NAME[] = L"FreeCam Phone Camera";

const AMOVIESETUP_MEDIATYPE s_pin_types[] = {{&MEDIATYPE_Video, &MEDIASUBTYPE_NULL}};

const AMOVIESETUP_PIN s_pins[] = {{
    const_cast<LPWSTR>(L"Output"), FALSE, TRUE, FALSE, FALSE,
    &CLSID_NULL, NULL, 1, s_pin_types,
}};

const REGFILTER2 s_reg_filter2 = {1, MERIT_DO_NOT_USE, 1, s_pins};

CUnknown* WINAPI CreateInstance(LPUNKNOWN lpunk, HRESULT* phr)
{
    return softcam::Softcam::CreateInstance(lpunk, CLSID_FreeCamPhone, phr);
}

HRESULT Register(bool reg)
{
    HRESULT hr = AMovieDllRegisterServer2(reg ? TRUE : FALSE);
    if (FAILED(hr)) return hr;
    hr = CoInitialize(nullptr);
    if (FAILED(hr)) return hr;
    IFilterMapper2* fm = nullptr;
    hr = CoCreateInstance(CLSID_FilterMapper2, nullptr, CLSCTX_INPROC_SERVER, IID_IFilterMapper2, (void**)&fm);
    if (SUCCEEDED(hr)) {
        fm->UnregisterFilter(&CLSID_VideoInputDeviceCategory, 0, CLSID_FreeCamPhone);
        if (reg) {
            hr = fm->RegisterFilter(CLSID_FreeCamPhone, FILTER_NAME, 0,
                                    &CLSID_VideoInputDeviceCategory, FILTER_NAME, &s_reg_filter2);
        }
        fm->Release();
    }
    CoFreeUnusedLibraries();
    CoUninitialize();
    return hr;
}

} // namespace

CFactoryTemplate g_Templates[] = {{FILTER_NAME, &CLSID_FreeCamPhone, &CreateInstance, NULL, nullptr}};
int g_cTemplates = sizeof(g_Templates) / sizeof(g_Templates[0]);

STDAPI DllRegisterServer() { return Register(true); }
STDAPI DllUnregisterServer() { return Register(false); }

extern "C" BOOL WINAPI DllEntryPoint(HINSTANCE, ULONG, LPVOID);

BOOL APIENTRY DllMain(HANDLE hModule, DWORD reason, LPVOID reserved)
{
    return DllEntryPoint((HINSTANCE)hModule, reason, reserved);
}
