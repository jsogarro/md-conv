---
title: "<script>alert('xss')</script>"
author: "\" onclick=\"alert('xss')"
description: "<img src=x onerror=alert('xss')>"
---
# Safe Content

This document tests XSS prevention in front matter fields.

The title, author, and description contain XSS payloads that should be escaped.
