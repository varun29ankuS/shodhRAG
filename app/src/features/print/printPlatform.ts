/** Whether the app writes PDF files itself (WebView2 on Windows); elsewhere the print dialog is used. */
export function writesPdfFiles(userAgent: string = typeof navigator === 'undefined' ? '' : navigator.userAgent): boolean {
  return /Windows/i.test(userAgent);
}
