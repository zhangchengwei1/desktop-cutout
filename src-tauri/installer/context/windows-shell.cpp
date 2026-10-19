// Explorer only gathers selected paths and launches the native application queue.
// No model loading, image work or waiting occurs inside Explorer.
#define UNICODE
#define _UNICODE
#include <windows.h>
#include <shlobj.h>
#include <shellapi.h>
#include <string>
#include <vector>
#include <new>
#include <cstdio>
#include <utility>

static HINSTANCE module;
static LONG objects = 0;
// {C6678B57-1379-4D98-9B5C-59933C6D42B2}
static const CLSID ClassId = {0xc6678b57,0x1379,0x4d98,{0x9b,0x5c,0x59,0x93,0x3c,0x6d,0x42,0xb2}};

static std::wstring label(UINT id) {
  wchar_t text[200] = {};
  LoadStringW(module, id, text, 200);
  return text;
}
static std::wstring quote(const std::wstring& value) {
  std::wstring result = L"\"";
  size_t slashes = 0;
  for (wchar_t ch : value) {
    if (ch == L'\\') { ++slashes; continue; }
    result.append(ch == L'"' ? slashes * 2 + 1 : slashes, L'\\');
    result += ch; slashes = 0;
  }
  result.append(slashes * 2, L'\\');
  return result + L"\"";
}
static bool image(const std::wstring& path) {
  DWORD attributes = GetFileAttributesW(path.c_str());
  if (attributes == INVALID_FILE_ATTRIBUTES || attributes & FILE_ATTRIBUTE_DIRECTORY) return false;
  size_t dot = path.find_last_of(L'.');
  if (dot == std::wstring::npos) return false;
  auto ext = path.substr(dot);
  return !_wcsicmp(ext.c_str(), L".png") || !_wcsicmp(ext.c_str(), L".jpg") || !_wcsicmp(ext.c_str(), L".jpeg") || !_wcsicmp(ext.c_str(), L".jpe") || !_wcsicmp(ext.c_str(), L".jfif") || !_wcsicmp(ext.c_str(), L".webp") || !_wcsicmp(ext.c_str(), L".gif") || !_wcsicmp(ext.c_str(), L".bmp") || !_wcsicmp(ext.c_str(), L".tif") || !_wcsicmp(ext.c_str(), L".tiff") || !_wcsicmp(ext.c_str(), L".ico") || !_wcsicmp(ext.c_str(), L".tga") || !_wcsicmp(ext.c_str(), L".dds") || !_wcsicmp(ext.c_str(), L".pnm") || !_wcsicmp(ext.c_str(), L".pbm") || !_wcsicmp(ext.c_str(), L".pgm") || !_wcsicmp(ext.c_str(), L".ppm") || !_wcsicmp(ext.c_str(), L".pam") || !_wcsicmp(ext.c_str(), L".qoi") || !_wcsicmp(ext.c_str(), L".ff");
}
static std::string jsonString(const std::wstring& path) {
  int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, path.data(), (int)path.size(), nullptr, 0, nullptr, nullptr);
  if (!size) return {};
  std::string utf8(size, '\0'), escaped = "\"";
  WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, path.data(), (int)path.size(), utf8.data(), size, nullptr, nullptr);
  for (unsigned char ch : utf8) {
    if (ch == '"' || ch == '\\') { escaped += '\\'; escaped += ch; }
    else if (ch < 32) { char buffer[7]; sprintf_s(buffer, "\\u%04x", ch); escaped += buffer; }
    else escaped += ch;
  }
  return escaped + "\"";
}

class ContextMenu final : public IShellExtInit, public IContextMenu {
  LONG references = 1;
  std::vector<std::wstring> paths;
public:
  ContextMenu() { InterlockedIncrement(&objects); }
  ~ContextMenu() { InterlockedDecrement(&objects); }
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** result) override {
    if (!result) return E_POINTER; *result = nullptr;
    if (iid == IID_IUnknown || iid == IID_IShellExtInit) *result = static_cast<IShellExtInit*>(this);
    else if (iid == IID_IContextMenu) *result = static_cast<IContextMenu*>(this);
    else return E_NOINTERFACE;
    AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&references); }
  ULONG STDMETHODCALLTYPE Release() override { LONG remaining = InterlockedDecrement(&references); if (!remaining) delete this; return remaining; }
  HRESULT STDMETHODCALLTYPE Initialize(PCIDLIST_ABSOLUTE folder, IDataObject* data, HKEY) override {
    try {
      paths.clear();
      if (data) {
        FORMATETC format = {CF_HDROP, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
        STGMEDIUM medium = {};
        if (FAILED(data->GetData(&format, &medium))) return E_INVALIDARG;
        struct MediumGuard {
          STGMEDIUM* value;
          ~MediumGuard() { ReleaseStgMedium(value); }
        } guard{&medium};
        HDROP drop = static_cast<HDROP>(medium.hGlobal);
        if (!drop) return E_INVALIDARG;
        UINT count = DragQueryFileW(drop, 0xffffffff, nullptr, 0);
        if (count > 10000) return E_INVALIDARG;
        for (UINT i = 0; i < count; ++i) {
          UINT length = DragQueryFileW(drop, i, nullptr, 0);
          std::wstring path(length + 1, L'\0'); DragQueryFileW(drop, i, path.data(), length + 1); path.resize(length);
          paths.push_back(std::move(path));
        }
      } else if (folder) {
        wchar_t path[32768];
        if (SHGetPathFromIDListEx(folder, path, 32768, GPFIDL_DEFAULT)) paths.emplace_back(path);
      }
      return paths.empty() ? E_INVALIDARG : S_OK;
    } catch (...) { return E_OUTOFMEMORY; }
  }
  HRESULT STDMETHODCALLTYPE QueryContextMenu(HMENU menu, UINT index, UINT first, UINT last, UINT flags) override {
    try {
      if (flags & CMF_DEFAULTONLY || paths.empty() || last < first + 1) return MAKE_HRESULT(SEVERITY_SUCCESS, 0, 0);
      for (const auto& path : paths) {
        DWORD attr = GetFileAttributesW(path.c_str());
        if (attr == INVALID_FILE_ATTRIBUTES || (!(attr & FILE_ATTRIBUTE_DIRECTORY) && !image(path))) return MAKE_HRESULT(SEVERITY_SUCCESS, 0, 0);
      }
      HMENU submenu = CreatePopupMenu();
      if (!submenu) return E_OUTOFMEMORY;
      AppendMenuW(submenu, MF_STRING | (paths.size() == 1 && image(paths[0]) ? 0 : MF_GRAYED), first, label(101).c_str());
      AppendMenuW(submenu, MF_STRING, first + 1, label(102).c_str());
      if (!InsertMenuW(menu, index, MF_BYPOSITION | MF_POPUP, reinterpret_cast<UINT_PTR>(submenu), L"ClearFrame")) { DestroyMenu(submenu); return E_FAIL; }
      return MAKE_HRESULT(SEVERITY_SUCCESS, 0, 2);
    } catch (...) { return E_OUTOFMEMORY; }
  }
  HRESULT STDMETHODCALLTYPE InvokeCommand(CMINVOKECOMMANDINFO* info) override {
    try {
      if (!info) return E_POINTER;
      UINT verb;
      auto extended = reinterpret_cast<CMINVOKECOMMANDINFOEX*>(info);
      bool unicode = info->cbSize >= sizeof(CMINVOKECOMMANDINFOEX) && (info->fMask & CMIC_MASK_UNICODE);
      if (unicode && HIWORD(extended->lpVerbW)) {
        if (!_wcsicmp(extended->lpVerbW, L"clearframe.single")) verb = 0;
        else if (!_wcsicmp(extended->lpVerbW, L"clearframe.batch")) verb = 1;
        else return E_INVALIDARG;
      }
      else if (!HIWORD(info->lpVerb)) verb = LOWORD(info->lpVerb);
      else if (!_stricmp(info->lpVerb, "clearframe.single")) verb = 0;
      else if (!_stricmp(info->lpVerb, "clearframe.batch")) verb = 1;
      else return E_INVALIDARG;
      if (verb > 1 || paths.empty() || (verb == 0 && (paths.size() != 1 || !image(paths[0])))) return E_INVALIDARG;
      wchar_t location[32768];
      DWORD size = GetModuleFileNameW(module, location, 32768);
      if (!size || size == 32768) return E_FAIL;
      std::wstring executable(location, size); executable.resize(executable.find_last_of(L"\\/") + 1); executable += L"clearframe-desktop.exe";
      GUID id; if (FAILED(CoCreateGuid(&id))) return E_FAIL;
      wchar_t guid[40], temp[32768]; StringFromGUID2(id, guid, 40);
      DWORD tempSize = GetTempPathW(32768, temp); if (!tempSize || tempSize >= 32768) return E_FAIL;
      std::wstring manifest = std::wstring(temp) + L"clearframe-selection-" + guid + L".json";
      std::string json = "[";
      for (const auto& path : paths) { auto entry = jsonString(path); if (entry.empty()) return E_INVALIDARG; if (json.size() > 1) json += ','; json += entry; }
      json += ']'; if (json.size() > 2 * 1024 * 1024) return E_INVALIDARG;
      HANDLE file = CreateFileW(manifest.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_TEMPORARY, nullptr);
      if (file == INVALID_HANDLE_VALUE) return HRESULT_FROM_WIN32(GetLastError());
      DWORD written = 0; BOOL saved = WriteFile(file, json.data(), (DWORD)json.size(), &written, nullptr); CloseHandle(file);
      if (!saved || written != json.size()) { DeleteFileW(manifest.c_str()); return E_FAIL; }
      std::wstring command = quote(executable) + L" --shell-cutout " + (verb == 0 ? L"single" : L"batch") + L" --selection-file " + quote(manifest);
      STARTUPINFOW startup = {}; startup.cb = sizeof(startup); PROCESS_INFORMATION process = {};
      if (!CreateProcessW(executable.c_str(), command.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW, nullptr, nullptr, &startup, &process)) { DWORD error = GetLastError(); DeleteFileW(manifest.c_str()); return HRESULT_FROM_WIN32(error); }
      CloseHandle(process.hThread); CloseHandle(process.hProcess); return S_OK;
    } catch (...) { return E_OUTOFMEMORY; }
  }
  HRESULT STDMETHODCALLTYPE GetCommandString(UINT_PTR command, UINT flags, UINT*, LPSTR text, UINT size) override {
    if (command > 1 || !text || !size) return E_INVALIDARG;
    if (flags == GCS_VERBW) return lstrcpynW(reinterpret_cast<LPWSTR>(text), command == 0 ? L"clearframe.single" : L"clearframe.batch", size) ? S_OK : E_FAIL;
    if (flags == GCS_VERBA) return lstrcpynA(text, command == 0 ? "clearframe.single" : "clearframe.batch", size) ? S_OK : E_FAIL;
    return E_NOTIMPL;
  }
};

class Factory final : public IClassFactory {
  LONG references = 1;
public:
  Factory() { InterlockedIncrement(&objects); }
  ~Factory() { InterlockedDecrement(&objects); }
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** result) override {
    if (!result) return E_POINTER; *result = nullptr;
    if (iid != IID_IUnknown && iid != IID_IClassFactory) return E_NOINTERFACE;
    *result = static_cast<IClassFactory*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&references); }
  ULONG STDMETHODCALLTYPE Release() override { LONG remaining = InterlockedDecrement(&references); if (!remaining) delete this; return remaining; }
  HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown* outer, REFIID iid, void** result) override {
    if (outer) return CLASS_E_NOAGGREGATION;
    auto instance = new(std::nothrow) ContextMenu; if (!instance) return E_OUTOFMEMORY;
    HRESULT hr = instance->QueryInterface(iid, result); instance->Release(); return hr;
  }
  HRESULT STDMETHODCALLTYPE LockServer(BOOL lock) override { if (lock) InterlockedIncrement(&objects); else InterlockedDecrement(&objects); return S_OK; }
};
extern "C" __declspec(dllexport) HRESULT __stdcall DllGetClassObject(REFCLSID clsid, REFIID iid, void** result) {
  if (clsid != ClassId) return CLASS_E_CLASSNOTAVAILABLE;
  auto factory = new(std::nothrow) Factory; if (!factory) return E_OUTOFMEMORY;
  HRESULT hr = factory->QueryInterface(iid, result); factory->Release(); return hr;
}
extern "C" __declspec(dllexport) HRESULT __stdcall DllCanUnloadNow() { return InterlockedCompareExchange(&objects, 0, 0) == 0 ? S_OK : S_FALSE; }
BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) { if (reason == DLL_PROCESS_ATTACH) { module = instance; DisableThreadLibraryCalls(instance); } return TRUE; }
