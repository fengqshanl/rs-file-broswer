# 第三方前端库

以下文件为未修改的第三方发行版，由服务端本地提供（不依赖外部 CDN）：

| 文件 | 项目 | 版本 | 许可 |
| --- | --- | --- | --- |
| `static/vendor/mammoth.browser.min.js` | [mammoth.js](https://github.com/mwilliamson/mammoth.js) | 1.8.0 | BSD-2-Clause |
| `static/vendor/xlsx.full.min.js` | [SheetJS](https://sheetjs.com/) | 0.20.3 | Apache-2.0 |

两者仅在用户打开 Word (.docx) / Excel (.xlsx, .xls, .csv) 预览时按需加载，解析全部在浏览器本地完成，文件内容不会发送到任何第三方服务。
