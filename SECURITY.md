# 🛡️ BÁO CÁO ĐÁNH GIÁ BẢO MẬT & QUYỀN RIÊNG TƯ (SECURITY & PRIVACY AUDIT)

Tài liệu này cung cấp bản kiểm toán bảo mật minh bạch và chi tiết về cách **Microsoft Teams Lite (Rust)** xử lý dữ liệu, xác thực người dùng, và giao tiếp mạng trong môi trường doanh nghiệp.

---

## 1. 🔍 Nguyên Tắc Cốt Lõi: Không Trung Gian, Không Telemetry

* **Mã Nguồn Mở 100%:** Toàn bộ mã nguồn được công khai, bất kỳ ai cũng có thể tự rà soát hoặc tự biên dịch trực tiếp từ máy của mình.
* **Không Có Máy Chủ Bên Thứ Ba:** Ứng dụng **hoàn toàn không có server backend trung gian**. Toàn bộ giao tiếp đều diễn ra trực tiếp giữa máy tính của bạn và hệ thống máy chủ chính thức của Microsoft.
* **Không Telemetry / Không Analytics:** Ứng dụng không thu thập thói quen sử dụng, không ghi nhật ký nội dung tin nhắn ra ngoài, và không có bất kỳ thư viện theo dõi (tracking) nào.

---

## 2. 🌐 Kiểm Toán Lưu Lượng Mạng (Network Endpoints Audit)

Ứng dụng chỉ kết nối tới các tên miền chính thức của Microsoft và GitHub (để kiểm tra cập nhật):

| Endpoint / URL | Giao Thức | Mục Đích | Bên Nhận |
| :--- | :---: | :--- | :---: |
| `login.microsoftonline.com` | HTTPS (TLS 1.3) | Lấy mã Device Code và trao đổi Access Token | Microsoft Identity |
| `graph.microsoft.com` | HTTPS (TLS 1.3) | Lấy họ tên và email người dùng để hiển thị trên UI | Microsoft Graph |
| `chatsvcagg.teams.microsoft.com` | HTTPS (TLS 1.3) | Tải danh sách hội thoại, lịch sử chat và gửi tin nhắn | Microsoft Teams |
| `go.trouter.teams.microsoft.com` | WSS (WebSocket Secure) | Nhận thông báo tin nhắn mới thời gian thực (Push Gateway) | Microsoft Trouter |
| `api.github.com` | HTTPS (TLS 1.3) | Kiểm tra phiên bản mới từ repo chính thức khi người dùng bấm `⇪` | GitHub |

---

## 3. 🔐 Cơ Chế Xác Thực & Màn Hình Phê Duyệt (Device Code Flow)

### Tại sao Microsoft hiển thị màn hình Approve khi đăng nhập?
Do Teams Lite là ứng dụng Desktop native (không nhúng trình duyệt Chrome/Edge nặng nề bên trong), ứng dụng sử dụng chuẩn xác thực **OAuth2 Device Code Flow** do Microsoft cung cấp cho các ứng dụng native/CLI.

Khi đăng nhập:
1. Ứng dụng gửi yêu cầu đến `login.microsoftonline.com` với Client ID chính thức của **Microsoft Teams** (`1fec8e78-bce4-4aaf-ab1b-5451cc387264`).
2. Mã xác thực được tạo và tự động mở trình duyệt tới trang chính thức `https://login.microsoft.com/device`.
3. Microsoft yêu cầu người dùng xác nhận trên trình duyệt: *"Bạn có muốn đăng nhập vào Microsoft Teams không?"*.  
   👉 **Đây là bước bảo vệ bình thường của Microsoft** nhằm đảm bảo chính bạn là người chủ động cấp quyền truy cập, tránh các cuộc tấn công lừa đảo (phishing).

### Danh mục quyền hạn (Scopes) được yêu cầu:
Ứng dụng áp dụng nguyên tắc **Đặc quyền tối thiểu (Principle of Least Privilege)**:
* `https://api.spaces.skype.com/.default`: Quyền nhắn tin nội bộ của hệ thống Teams (`chatsvc`).
* `offline_access`: Cho phép cấp Refresh Token để tự động duy trì phiên đăng nhập mà không bắt người dùng nhập mã liên tục mỗi giờ.
* `openid profile email`: Để đọc họ tên và email hiển thị trên thanh tiêu đề ứng dụng.
* ❌ **Tuyệt đối không yêu cầu:** Quyền quản trị viên (Admin consent), quyền đọc tệp SharePoint/OneDrive toàn công ty, hoặc quyền can thiệp vào máy chủ doanh nghiệp.

---

## 4. 🗄️ Lưu Trữ Dữ Liệu & Quản Lý Token

* **Mã Hóa Cấp Hệ Điều Hành:** Token đăng nhập (`access_token` và `refresh_token`) **không bao giờ lưu dưới dạng file text thông thường**.
* **Tích Hợp Windows Credential Manager:** Ứng dụng sử dụng crate `keyring` để lưu token trực tiếp vào kho lưu trữ chứng chỉ an toàn của Windows, được mã hóa bằng **Windows Data Protection API (DPAPI)**.
* **Cô Lập Phiên Người Dùng:** Token chỉ có thể được giải mã bởi chính tài khoản Windows đang đăng nhập trên máy tính. Các tài khoản người dùng khác trên cùng máy tính không thể truy cập.
* **Xóa Sạch Khi Đăng Xuất (Logout):** Khi người dùng chọn đăng xuất, chứng chỉ trong Windows Credential Manager sẽ bị xóa hoàn toàn.

---

## 5. 🏢 Sử Dụng Trong Môi Trường Doanh Nghiệp (Enterprise & Corporate IT)

### Quản trị viên IT của công ty sẽ thấy gì?
Khi bạn đăng nhập vào Teams Lite từ tài khoản công ty (ví dụ tài khoản Office 365 / Microsoft Entra ID của công ty):
* **Nhật ký đăng nhập (Azure AD Sign-in Logs):**
  * Tên ứng dụng: `Microsoft Teams` (App ID: `1fec8e78-bce4-4aaf-ab1b-5451cc387264`).
  * Giao thức xác thực: `Device Code`.
  * Địa chỉ IP: IP mạng công ty hoặc IP thiết bị của bạn.
* **Tính tuân thủ (Compliance):** 
  * Về mặt giao thức, Microsoft ghi nhận đây là phiên làm việc hợp lệ của Microsoft Teams.
  * *Lưu ý:* Một số tập đoàn tài chính/ngân hàng áp dụng chính sách ngặt nghèo **Conditional Access** (yêu cầu thiết bị phải cài đặt chứng chỉ quản lý Microsoft Intune Compliant). Nếu tenant của công ty bạn khóa hoàn toàn luồng Device Code, việc đăng nhập có thể bị chặn bởi chính sách này.

---

## 6. 🚨 Báo Cáo Lỗ Hổng Bảo Mật (Vulnerability Reporting)

Nếu bạn phát hiện bất kỳ vấn đề bảo mật nào trong mã nguồn:
* Vui lòng tạo Issue riêng tư hoặc liên hệ trực tiếp qua GitHub: [https://github.com/thichcode/teams-rust-lite](https://github.com/thichcode/teams-rust-lite).
* Chúng tôi cam kết xử lý và phản hồi nhanh chóng trong vòng 24 - 48 giờ.
